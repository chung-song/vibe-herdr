//! Mic/network-free replay of the actual worker, VAD, resampler and delivery.
//! Only the expensive STT and remote judge are replaced. Herdr is a recording
//! executable; no real pane, microphone, classifier credit or config is used.
#![allow(dead_code)]

#[path = "../src/accent.rs"]
mod accent;
#[path = "../src/config.rs"]
mod config;
#[path = "../src/control.rs"]
mod control;
#[path = "../src/deliver.rs"]
mod deliver;
#[path = "../src/dictation.rs"]
mod dictation;
#[path = "../src/models.rs"]
mod models;
#[path = "../src/turn_gate.rs"]
mod real_turn_gate;
#[path = "../src/silero.rs"]
mod silero;
#[path = "../src/vad.rs"]
mod vad;

fn stamp() -> String {
    "replay".into()
}

mod turn_gate {
    pub(crate) use crate::real_turn_gate::{parse_jev_verdict, safe_verdict};
    pub fn judge(_: &str) -> Result<bool, String> {
        Ok(false)
    }
}

mod stt {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    pub static FED: AtomicUsize = AtomicUsize::new(0);
    pub static DECODING: AtomicBool = AtomicBool::new(false);
    pub static HOLD: AtomicBool = AtomicBool::new(false);
    pub static HOLD_LOAD: AtomicBool = AtomicBool::new(false);
    pub struct Stt;
    pub struct Stream<'a> {
        stt: &'a Stt,
        audio: Vec<f32>,
    }
    impl Stt {
        pub fn shared() -> std::io::Result<&'static Self> {
            while HOLD_LOAD.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            Ok(&Stt)
        }
        pub fn stream(&self) -> Stream<'_> {
            Stream {
                stt: self,
                audio: Vec::new(),
            }
        }
        pub fn transcribe(&self, audio: &[f32]) -> std::io::Result<String> {
            DECODING.store(true, Ordering::SeqCst);
            while HOLD.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            let parts: Vec<_> = [(0.25, "첫 문장"), (0.35, "뒤 문장"), (0.5, "새 문장")]
                .into_iter()
                .filter(|(level, _)| audio.contains(level))
                .map(|(_, text)| text)
                .collect();
            Ok(format!("{} 이상", parts.join(" ")))
        }
    }
    impl Stream<'_> {
        pub fn feed(&mut self, audio: &[f32]) -> std::io::Result<()> {
            self.audio.extend_from_slice(audio);
            FED.fetch_add(audio.len(), Ordering::SeqCst);
            Ok(())
        }
        pub fn finish(self) -> std::io::Result<String> {
            self.stt.transcribe(&self.audio)
        }
    }
}

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

fn wait_for(message: &str, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(4);
    while !predicate() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(predicate(), "{message}");
}

// Each case runs in its own process: config OnceLock and environment cannot
// collide with the production modules' unit tests under cargo's parallel run.
#[test]
fn worker_focus_during_speech_preserves_whole_sentence() {
    replay("worker_focus_during_speech_preserves_whole_sentence");
}

#[test]
fn worker_queued_onset_keeps_capture_pane() {
    replay("worker_queued_onset_keeps_capture_pane");
}

#[test]
fn worker_phase_tracks_voice_not_end_of_speech_debounce() {
    replay("worker_phase_tracks_voice_not_end_of_speech_debounce");
}

#[test]
fn worker_switch_after_end_keeps_next_pane_separate() {
    replay("worker_switch_after_end_keeps_next_pane_separate");
}

#[test]
fn worker_waiting_is_orange_and_draft_survives_switch() {
    replay("worker_waiting_is_orange_and_draft_survives_switch");
}

#[test]
fn worker_focus_during_transcription_preserves_queued_panes() {
    replay("worker_focus_during_transcription_preserves_queued_panes");
}

#[test]
fn worker_focus_at_submission_keeps_the_original_destination() {
    replay("worker_focus_at_submission_keeps_the_original_destination");
}

fn replay(case: &str) {
    if std::env::var("HANDSFREE_REPLAY_CASE").ok().as_deref() != Some(case) {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", case, "--nocapture"])
            .env("HANDSFREE_REPLAY_CASE", case)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("worker-replay-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("herdr");
    let focus = dir.join("focus");
    let sent = dir.join("sent");
    std::fs::write(&focus, "pane-A").unwrap();
    let waiting = case == "worker_waiting_is_orange_and_draft_survives_switch";
    let decoding = case == "worker_focus_during_transcription_preserves_queued_panes";
    let submit_race = case == "worker_focus_at_submission_keeps_the_original_destination";
    std::fs::write(
        dir.join("config.toml"),
        if waiting {
            "vad = \"energy\"\n"
        } else {
            "vad = \"energy\"\nend_word = \"이상\"\n"
        },
    )
    .unwrap();
    std::fs::write(&script, format!(
        "#!/bin/sh\nif [ \"$1 $2\" = 'pane list' ]; then printf '{{\"result\":{{\"panes\":[{{\"pane_id\":\"%s\",\"focused\":true,\"agent\":\"hermes\"}}]}}}}' \"$(/bin/cat '{0}')\"; if [ \"$HANDSFREE_REPLAY_SUBMIT_RACE\" = 1 ]; then printf pane-B > '{0}'; fi; elif [ \"$1 $2\" = 'agent prompt' ]; then printf '%s|%s\\n' \"$3\" \"$4\" >> '{1}'; else exit 2; fi\n", focus.display(), sent.display()
    )).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    unsafe {
        std::env::set_var("HERDR_BIN_PATH", &script);
        std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", &dir);
        std::env::set_var("HANDSFREE_VAD", "energy");
        std::env::set_var(
            "HANDSFREE_REPLAY_SUBMIT_RACE",
            if submit_race { "1" } else { "0" },
        );
    }
    let stop = Arc::new(AtomicBool::new(false));
    let tracker = Arc::new(deliver::FocusTracker::new(Some("pane-A".into())));
    let telemetry = Arc::new(dictation::Telemetry::default());
    let (tx, rx) = mpsc::channel();
    let queued = case == "worker_queued_onset_keeps_capture_pane";
    let phases = case == "worker_phase_tracks_voice_not_end_of_speech_debounce";
    let after_end = case == "worker_switch_after_end_keeps_next_pane_separate";
    stt::HOLD_LOAD.store(queued, Ordering::SeqCst);
    let worker = {
        let (stop, tracker, telemetry) = (stop.clone(), tracker.clone(), telemetry.clone());
        std::thread::spawn(move || dictation::worker_loop(&stop, rx, &telemetry, &tracker))
    };
    tx.send(dictation::AudioChunk::captured(vec![16_000.0], &tracker))
        .unwrap();
    let send = |level, frames| {
        for _ in 0..frames {
            tx.send(dictation::AudioChunk::captured(
                vec![level; vad::FRAME],
                &tracker,
            ))
            .unwrap();
        }
    };
    send(0.25, 16);
    if queued {
        send(0.35, 12);
        send(0.0, 70);
    } else {
        wait_for("worker did not consume speech onset", || {
            stt::FED.load(Ordering::SeqCst) >= 16 * vad::FRAME
        });
        assert_eq!(telemetry.phase(), "speech");
    }
    if phases {
        assert_eq!(accent::color_for(telemetry.phase()), Some("#f7768e"));
        send(0.0, 1);
        wait_for("first silent frame must stop reporting speech", || {
            telemetry.phase() != "speech"
        });
        assert_eq!(telemetry.phase(), "settling");
        assert_eq!(accent::color_for(telemetry.phase()), Some("#ff9e64"));
        send(0.0, 11); // 384 ms natural pause: no transcription/submission.
        wait_for("pause not consumed", || {
            stt::FED.load(Ordering::SeqCst) >= 28 * vad::FRAME
        });
        assert!(!stt::DECODING.load(Ordering::SeqCst));
        assert!(!sent.exists());
        send(0.35, 12);
        wait_for("resumed voice not red", || telemetry.phase() == "speech");
        stt::HOLD.store(true, Ordering::SeqCst);
        send(0.0, 70);
        wait_for("STT did not start", || stt::DECODING.load(Ordering::SeqCst));
        assert_eq!(telemetry.phase(), "transcribing");
        assert_eq!(accent::color_for(telemetry.phase()), Some("#ff9e64"));
        assert!(!sent.exists());
        stt::HOLD.store(false, Ordering::SeqCst);
    }
    if waiting || decoding {
        stt::HOLD.store(decoding, Ordering::SeqCst);
        send(0.35, 12);
        send(0.0, 70);
        let expected = if waiting { "waiting" } else { "transcribing" };
        wait_for("processing phase not visible", || {
            telemetry.phase() == expected
        });
        assert_eq!(accent::color_for(telemetry.phase()), Some("#ff9e64"));
        assert!(!sent.exists(), "draft submitted prematurely");
    }
    if after_end {
        send(0.35, 12);
        send(0.0, 1);
        wait_for("end of speech not consumed", || {
            stt::FED.load(Ordering::SeqCst) >= 29 * vad::FRAME
        });
    }
    if !phases && !submit_race {
        std::fs::write(&focus, "pane-B").unwrap();
        tracker.observe(Some("pane-B".into()));
    }
    if queued {
        stt::HOLD_LOAD.store(false, Ordering::SeqCst);
    } else if after_end || decoding {
        send(0.5, 16);
        send(0.0, 70);
        if decoding {
            std::fs::write(&focus, "pane-C").unwrap();
            tracker.observe(Some("pane-C".into()));
            stt::HOLD.store(false, Ordering::SeqCst);
        }
    } else if waiting {
        send(0.0, 2);
    } else if !phases {
        send(0.35, 12);
        send(0.0, 70);
    }
    drop(tx);
    worker.join().unwrap();
    stop.store(true, Ordering::SeqCst);
    let deliveries = std::fs::read_to_string(&sent).unwrap_or_default();
    assert!(
        deliveries.contains("pane-A|첫 문장 뒤 문장"),
        "in-flight audio was lost or split: {deliveries:?}"
    );
    if after_end || decoding {
        assert_eq!(
            deliveries.lines().count(),
            2,
            "expected separate turns: {deliveries:?}"
        );
        assert!(
            deliveries
                .lines()
                .any(|line| line.trim_end_matches(" 이상") == "pane-B|새 문장"),
            "new speech mixed into original pane: {deliveries:?}"
        );
        assert!(
            !deliveries
                .lines()
                .any(|line| line.starts_with("pane-A|") && line.contains("새 문장"))
        );
    } else {
        assert!(
            !deliveries.contains("pane-B|"),
            "old speech reached new pane: {deliveries:?}"
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}
