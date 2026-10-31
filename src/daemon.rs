//! Long-running daemon: owns mic + camera (later milestones) and answers
//! control requests over the unix socket.

use crate::control::{self, DictationMode, Request, Response, State, Target};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};

pub struct Daemon {
    pub state: Mutex<State>,
    dictation: Mutex<Option<DictationEngine>>,
    gaze: Mutex<Option<crate::gaze::Gaze>>,
}

enum DictationEngine {
    Auto(crate::dictation::Dictation),
    Space(crate::push_to_talk::PushToTalk),
}

impl DictationEngine {
    fn phase(&self) -> &'static str {
        match self {
            Self::Auto(d) => d.phase(),
            Self::Space(d) => d.phase(),
        }
    }

    fn try_stop_space(&self) -> bool {
        match self {
            Self::Space(d) => d.try_stop(),
            Self::Auto(_) => true,
        }
    }
}

pub fn run() -> std::io::Result<()> {
    let dir = control::state_dir();
    std::fs::create_dir_all(&dir)?;
    let sock = control::socket_path();

    // Refuse to double-start; replace a stale socket from a dead daemon.
    if control::daemon_alive() {
        eprintln!(
            "herdr-handsfree daemon already running at {}",
            sock.display()
        );
        return Ok(());
    }
    let _ = std::fs::remove_file(&sock);

    let listener = UnixListener::bind(&sock)?;
    eprintln!(
        "[{}] herdr-handsfree daemon listening on {}",
        crate::stamp(),
        sock.display()
    );

    let daemon = Arc::new(Daemon {
        state: Mutex::new(State {
            dictation_mode: match std::fs::read_to_string(dir.join("dictation-mode")) {
                Ok(s) if s.trim() == "space" => DictationMode::Space,
                _ => DictationMode::Auto,
            },
            ..State::default()
        }),
        dictation: Mutex::new(None),
        gaze: Mutex::new(None),
    });

    // Recover only our marked line after an unclean exit, even if the option
    // was turned off before this restart.
    crate::accent::apply(None);
    let watcher = if crate::config::get().phase_accent {
        let watched = daemon.clone();
        Some(std::thread::spawn(move || {
            let mut shown = None;
            while !watched.stopped() {
                let phase = watched
                    .dictation
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|d| d.phase())
                    .unwrap_or("off");
                let enabled = watched.state.lock().unwrap().dictation;
                let color = accent_for_dictation(enabled, phase);
                if color != shown {
                    crate::accent::apply(color);
                    shown = color;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }))
    } else {
        None
    };

    for conn in listener.incoming() {
        let Ok(stream) = conn else { continue };
        let worker = daemon.clone();
        std::thread::spawn(move || {
            let _ = handle(&worker, stream);
        });
        if daemon.stopped() {
            break;
        }
    }
    let _ = std::fs::remove_file(&sock);
    // The watcher must finish before cleanup or it could reinsert a tint.
    if let Some(watcher) = watcher {
        let _ = watcher.join();
    }
    crate::accent::apply(None);
    Ok(())
}

fn accent_for_dictation(enabled: bool, phase: &str) -> Option<&'static str> {
    enabled.then(|| crate::accent::color_for(phase)).flatten()
}

#[cfg(test)]
mod accent_tests {
    use super::accent_for_dictation;

    #[test]
    fn accent_only_follows_phase_while_dictation_is_on() {
        assert_eq!(accent_for_dictation(false, "speech"), None);
        assert_eq!(accent_for_dictation(true, "speech"), Some("#f7768e"));
        assert_eq!(accent_for_dictation(true, "checking"), Some("#ff9e64"));
        assert_eq!(accent_for_dictation(false, "checking"), None);
        assert_eq!(accent_for_dictation(true, "off"), None);
    }
}

impl Daemon {
    fn stopped(&self) -> bool {
        STOP.load(std::sync::atomic::Ordering::SeqCst)
    }
}

static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn handle(daemon: &Daemon, stream: UnixStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let resp = match serde_json::from_str::<Request>(&line) {
        Ok(req) => dispatch(daemon, req),
        Err(e) => Response {
            ok: false,
            state: daemon.state.lock().unwrap().clone(),
            error: Some(format!("bad request: {e}")),
            message: None,
        },
    };
    let mut out = serde_json::to_string(&resp).expect("serialize response");
    out.push('\n');
    let mut stream = stream;
    stream.write_all(out.as_bytes())
}

fn dispatch(daemon: &Daemon, req: Request) -> Response {
    let mut message = None;
    let mut error = None;
    match req {
        Request::Ping | Request::Status => {}
        Request::CheckSpacePermission => {
            if !crate::push_to_talk::keyboard_access_available() {
                crate::push_to_talk::request_keyboard_access();
                error = Some(
                    "macOS Accessibility permission is required for space dictation".to_string(),
                );
            }
        }
        Request::Toggle { target } => {
            let mut state = daemon.state.lock().unwrap();
            match target {
                Target::Dictation => {
                    let busy = state.dictation
                        && state.dictation_mode == DictationMode::Space
                        && daemon
                            .dictation
                            .lock()
                            .unwrap()
                            .as_ref()
                            .is_some_and(|d| !d.try_stop_space());
                    if busy {
                        error = Some(
                            "space dictation is recording or transcribing; wait for delivery"
                                .to_string(),
                        );
                    } else {
                        state.dictation = !state.dictation;
                    }
                }
                Target::Gaze => state.gaze = !state.gaze,
            }
            apply(daemon, &state);
        }
        Request::CycleDictationMode => {
            let mut state = daemon.state.lock().unwrap();
            if state.dictation {
                error = Some("turn dictation off before switching modes".to_string());
            } else {
                let next = state.dictation_mode.next();
                if next == DictationMode::Space && !crate::push_to_talk::keyboard_access_available()
                {
                    crate::push_to_talk::request_keyboard_access();
                    let exe = std::env::current_exe()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| "herdr-handsfree".to_string());
                    error = Some(format!(
                        "Enable Accessibility for {exe} in macOS System Settings, then switch modes again"
                    ));
                } else {
                    let path = control::state_dir().join("dictation-mode");
                    let tmp = path.with_extension("tmp");
                    match std::fs::write(&tmp, next.as_str())
                        .and_then(|()| std::fs::rename(&tmp, &path))
                    {
                        Ok(()) => state.dictation_mode = next,
                        Err(e) => error = Some(format!("cannot save dictation mode: {e}")),
                    }
                }
            }
        }
        Request::Stop => {
            let state = daemon.state.lock().unwrap();
            let busy = state.dictation_mode == DictationMode::Space
                && daemon
                    .dictation
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|d| !d.try_stop_space());
            if busy {
                error = Some(
                    "space dictation is recording or transcribing; wait for delivery".to_string(),
                );
            } else {
                STOP.store(true, std::sync::atomic::Ordering::SeqCst);
                // The accept loop notices on the next connection; nudge it.
                std::thread::spawn(|| {
                    let _ = UnixStream::connect(control::socket_path());
                });
            }
        }
        // Lock order everywhere: state before gaze (apply() holds state while
        // locking gaze, so any gaze→state nesting would deadlock).
        Request::CalStart => {
            let mut state = daemon.state.lock().unwrap();
            // The camera turns on for calibration; surface that in the flag so
            // an aborted calibration can be seen and toggled off.
            state.gaze = true;
            let mut gaze = daemon.gaze.lock().unwrap();
            let gaze = gaze.get_or_insert_with(|| crate::gaze::Gaze::start(false));
            gaze.shared
                .move_enabled
                .store(false, std::sync::atomic::Ordering::SeqCst);
            gaze.samples.lock().unwrap().clear();
        }
        Request::CalTarget { index } => {
            let [x, y] = crate::gaze::calibration_point(index);
            if let Err(e) = crate::gaze::mouse::move_cursor(x, y) {
                error = Some(e);
            }
        }
        Request::CalCapture { index } => {
            let gaze = daemon.gaze.lock().unwrap();
            match gaze.as_ref().and_then(|g| g.capture_features()) {
                Some(features) => {
                    let point = crate::gaze::calibration_point(index);
                    gaze.as_ref()
                        .unwrap()
                        .samples
                        .lock()
                        .unwrap()
                        .push((features, point));
                }
                None => {
                    error =
                        Some("no face detected yet — face the camera and try again".to_string());
                }
            }
        }
        Request::CalFinish => {
            let mut state = daemon.state.lock().unwrap();
            let gaze = daemon.gaze.lock().unwrap();
            match gaze.as_ref() {
                Some(g) => match g.finish_calibration() {
                    Ok(rms) => {
                        message = Some(format!("calibrated, fit rms {rms:.0} px"));
                        // Calibration implies the user wants gaze moves on.
                        g.shared
                            .move_enabled
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                        state.gaze = true;
                    }
                    Err(e) => error = Some(e),
                },
                None => error = Some("calibration not started".to_string()),
            }
        }
    }
    let mut state = daemon.state.lock().unwrap().clone();
    state.dictation_phase = daemon
        .dictation
        .lock()
        .unwrap()
        .as_ref()
        .map(|d| d.phase())
        .unwrap_or("off")
        .to_string();
    state.gaze_calibrated = daemon
        .gaze
        .lock()
        .unwrap()
        .as_ref()
        .map(|g| g.is_calibrated())
        .unwrap_or_else(|| {
            crate::control::state_dir()
                .join("gaze-mapping.json")
                .exists()
        });
    Response {
        ok: error.is_none(),
        state,
        error,
        message,
    }
}

/// React to state changes: start/stop the pipelines to match the flags.
fn apply(daemon: &Daemon, state: &State) {
    eprintln!("state: dictation={} gaze={}", state.dictation, state.gaze);
    let mut dictation = daemon.dictation.lock().unwrap();
    match (state.dictation, dictation.is_some()) {
        (true, false) => {
            *dictation = Some(match state.dictation_mode {
                DictationMode::Auto => DictationEngine::Auto(crate::dictation::Dictation::start()),
                DictationMode::Space => {
                    DictationEngine::Space(crate::push_to_talk::PushToTalk::start())
                }
            })
        }
        (false, true) => *dictation = None,
        _ => {}
    }
    let mut gaze = daemon.gaze.lock().unwrap();
    match (state.gaze, gaze.as_ref()) {
        (true, None) => *gaze = Some(crate::gaze::Gaze::start(true)),
        (true, Some(g)) => g
            .shared
            .move_enabled
            .store(true, std::sync::atomic::Ordering::SeqCst),
        // Drop the pipeline entirely so the camera turns off.
        (false, Some(_)) => *gaze = None,
        (false, None) => {}
    }
}
