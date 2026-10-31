//! Opt-in push-to-talk. Only an unmodified space in the configured terminal
//! is intercepted; audio is captured from key-down to key-up and is sent to
//! the agent pane captured at key-down, without the remote completion judge.
#![allow(unexpected_cfgs)]

use crate::{deliver, dictation, stt, vad};
use core_foundation::runloop::{CFRunLoop, kCFRunLoopDefaultMode};
use core_foundation::{
    base::TCFType, boolean::CFBoolean, dictionary::CFDictionary, string::CFString,
};
use core_graphics::event::{
    CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, CallbackResult, EventField,
};
use cpal::traits::{DeviceTrait, StreamTrait};
use objc::{class, msg_send, runtime::Object, sel, sel_impl};
use std::ffi::CStr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    static kAXTrustedCheckOptionPrompt: core_foundation::string::CFStringRef;
    fn AXIsProcessTrustedWithOptions(options: core_foundation::dictionary::CFDictionaryRef)
    -> bool;
}

struct Shared {
    stop: AtomicBool,
    error: AtomicBool,
    tap_ready: AtomicBool,
    mic_ready: AtomicBool,
    held: AtomicBool,
    sequence: AtomicU64,
    recording: AtomicBool,
    pending: AtomicUsize,
    transition: Mutex<()>,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            stop: AtomicBool::new(false),
            error: AtomicBool::new(false),
            tap_ready: AtomicBool::new(false),
            mic_ready: AtomicBool::new(false),
            held: AtomicBool::new(false),
            sequence: AtomicU64::new(0),
            recording: AtomicBool::new(false),
            pending: AtomicUsize::new(0),
            transition: Mutex::new(()),
        }
    }
}

pub struct PushToTalk(Arc<Shared>);

enum KeyEvent {
    Press(String, u64),
    Release,
    Cancel,
}

#[derive(Debug, PartialEq, Eq)]
enum SpaceAction {
    Pass,
    Begin,
    SuppressRepeat,
    End,
}

fn space_action(down: bool, held: bool, eligible: bool) -> SpaceAction {
    match (down, held, eligible) {
        (true, true, _) => SpaceAction::SuppressRepeat,
        (true, false, true) => SpaceAction::Begin,
        (false, true, _) => SpaceAction::End,
        _ => SpaceAction::Pass,
    }
}

struct Recording {
    enabled: bool,
    samples: Vec<f32>,
}

impl Recording {
    fn begin(&mut self) {
        self.samples.clear();
        self.enabled = true;
    }

    fn capture(&mut self, data: &[f32], channels: usize, gain: f32) {
        if self.enabled {
            self.samples.extend(data.chunks(channels).map(|frame| {
                (frame.iter().sum::<f32>() / channels as f32 * gain).clamp(-1.0, 1.0)
            }));
        }
    }

    fn finish(&mut self) -> Vec<f32> {
        self.enabled = false;
        std::mem::take(&mut self.samples)
    }
}

struct Job {
    pane: String,
    samples: Vec<f32>,
    sample_rate: usize,
}

fn transcribe_and_deliver(stt: &stt::Stt, job: Job) -> Result<(), String> {
    let mut resampler = vad::Resampler::new(job.sample_rate);
    let audio = resampler.push(&job.samples);
    if audio.len() < vad::SAMPLE_RATE / 4 {
        return Ok(());
    }
    let text = stt.transcribe(&audio).map_err(|e| e.to_string())?;
    let text = crate::config::correct_transcript(&text);
    if !text.trim().is_empty() {
        let pane = deliver::FocusedPane { pane_id: job.pane };
        deliver::deliver_to_original(&pane, &text)?;
    }
    Ok(())
}

impl PushToTalk {
    pub fn start() -> Self {
        let shared = Arc::new(Shared::default());
        let (tx, rx) = mpsc::channel();
        let focus_stop = Arc::new(AtomicBool::new(false));
        let tracker = deliver::watch_focus(focus_stop.clone());
        let tap_shared = shared.clone();
        let tap_tx = tx.clone();
        std::thread::spawn(move || {
            if let Err(e) = key_monitor(&tap_shared, &tap_tx, &tracker) {
                eprintln!("push-to-talk keyboard error: {e}");
                tap_shared.error.store(true, Ordering::SeqCst);
                let _ = tap_tx.send(KeyEvent::Cancel);
            }
            focus_stop.store(true, Ordering::SeqCst);
        });
        let mic_shared = shared.clone();
        std::thread::spawn(move || {
            if let Err(e) = record_loop(&mic_shared, rx) {
                eprintln!("push-to-talk audio error: {e}");
                mic_shared.error.store(true, Ordering::SeqCst);
            }
        });
        Self(shared)
    }

    pub fn phase(&self) -> &'static str {
        if self.0.error.load(Ordering::SeqCst) {
            "error"
        } else if self.0.held.load(Ordering::SeqCst) && !self.0.recording.load(Ordering::SeqCst) {
            "preparing"
        } else if self.0.held.load(Ordering::SeqCst) {
            "speech"
        } else if self.0.pending.load(Ordering::SeqCst) > 0 {
            "transcribing"
        } else if self.0.tap_ready.load(Ordering::SeqCst) && self.0.mic_ready.load(Ordering::SeqCst)
        {
            "listening"
        } else {
            "preparing"
        }
    }

    pub fn try_stop(&self) -> bool {
        let Ok(_guard) = self.0.transition.lock() else {
            return false;
        };
        if self.0.held.load(Ordering::SeqCst) || self.0.pending.load(Ordering::SeqCst) > 0 {
            return false;
        }
        self.0.stop.store(true, Ordering::SeqCst);
        true
    }
}

impl Drop for PushToTalk {
    fn drop(&mut self) {
        self.0.stop.store(true, Ordering::SeqCst);
    }
}

/// Identify the frontmost application without inspecting its content.
fn frontmost_bundle() -> Option<String> {
    objc::rc::autoreleasepool(|| unsafe {
        let workspace: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
        if workspace.is_null() {
            return None;
        }
        let app: *mut Object = msg_send![workspace, frontmostApplication];
        if app.is_null() {
            return None;
        }
        let bundle: *mut Object = msg_send![app, bundleIdentifier];
        if bundle.is_null() {
            return None;
        }
        let utf8: *const std::ffi::c_char = msg_send![bundle, UTF8String];
        if utf8.is_null() {
            return None;
        }
        Some(CStr::from_ptr(utf8).to_string_lossy().into_owned())
    })
}

/// Probe the actual daemon executable's permission without enabling a tap or
/// consuming keyboard events. The system may show its accessibility prompt.
pub fn keyboard_access_available() -> bool {
    CGEventTap::new(
        CGEventTapLocation::Session,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        vec![CGEventType::KeyDown, CGEventType::KeyUp],
        |_, _, _| CallbackResult::Keep,
    )
    .is_ok()
}

pub fn request_keyboard_access() {
    unsafe {
        let key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
        let options = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::true_value())]);
        AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef());
    }
}

fn key_monitor(
    shared: &Arc<Shared>,
    tx: &mpsc::Sender<KeyEvent>,
    tracker: &deliver::FocusTracker,
) -> Result<(), String> {
    let event_shared = shared.clone();
    let event_tx = tx.clone();
    let bundle = crate::config::get().push_to_talk_app.clone();
    CGEventTap::with_enabled(
        CGEventTapLocation::Session,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        vec![CGEventType::KeyDown, CGEventType::KeyUp],
        |_, event_type, event| {
            if matches!(
                event_type,
                CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
            ) {
                event_shared.error.store(true, Ordering::SeqCst);
                return CallbackResult::Keep;
            }
            if event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) != 49 {
                return CallbackResult::Keep;
            }
            match event_type {
                CGEventType::KeyDown => {
                    if space_action(true, event_shared.held.load(Ordering::SeqCst), false)
                        == SpaceAction::SuppressRepeat
                    {
                        return CallbackResult::Drop;
                    }
                    let mods = CGEventFlags::CGEventFlagShift
                        | CGEventFlags::CGEventFlagControl
                        | CGEventFlags::CGEventFlagAlternate
                        | CGEventFlags::CGEventFlagCommand;
                    let eligible = event_shared.mic_ready.load(Ordering::SeqCst)
                        && !event_shared.error.load(Ordering::SeqCst)
                        && !event.get_flags().intersects(mods)
                        && frontmost_bundle().as_deref() == Some(bundle.as_str());
                    if space_action(true, false, eligible) != SpaceAction::Begin {
                        return CallbackResult::Keep;
                    }
                    let (_, pane) = tracker.snapshot();
                    let Some(pane) = pane else {
                        return CallbackResult::Keep;
                    };
                    let Ok(_guard) = event_shared.transition.lock() else {
                        return CallbackResult::Keep;
                    };
                    if event_shared.stop.load(Ordering::SeqCst) {
                        return CallbackResult::Keep;
                    }
                    let sequence = event_shared.sequence.fetch_add(1, Ordering::SeqCst) + 1;
                    event_shared.held.store(true, Ordering::SeqCst);
                    if event_tx.send(KeyEvent::Press(pane, sequence)).is_err() {
                        event_shared.held.store(false, Ordering::SeqCst);
                        return CallbackResult::Keep;
                    }
                    CallbackResult::Drop
                }
                CGEventType::KeyUp
                    if space_action(false, event_shared.held.load(Ordering::SeqCst), false)
                        == SpaceAction::End =>
                {
                    let Ok(_guard) = event_shared.transition.lock() else {
                        return CallbackResult::Keep;
                    };
                    // Reserve pending before clearing the held phase so a
                    // mode switch cannot cancel an already released turn.
                    event_shared.pending.fetch_add(1, Ordering::SeqCst);
                    event_shared.held.store(false, Ordering::SeqCst);
                    if event_tx.send(KeyEvent::Release).is_err() {
                        event_shared.pending.fetch_sub(1, Ordering::SeqCst);
                    }
                    CallbackResult::Drop
                }
                _ => CallbackResult::Keep,
            }
        },
        || {
            shared.tap_ready.store(true, Ordering::SeqCst);
            while !shared.stop.load(Ordering::SeqCst) && !shared.error.load(Ordering::SeqCst) {
                CFRunLoop::run_in_mode(
                    unsafe { kCFRunLoopDefaultMode },
                    Duration::from_millis(100),
                    true,
                );
            }
            if shared.held.load(Ordering::SeqCst) {
                shared.pending.fetch_add(1, Ordering::SeqCst);
                shared.held.store(false, Ordering::SeqCst);
                if tx.send(KeyEvent::Release).is_err() {
                    shared.pending.fetch_sub(1, Ordering::SeqCst);
                }
            }
            let _ = tx.send(KeyEvent::Cancel);
            shared.tap_ready.store(false, Ordering::SeqCst);
        },
    )
    .map_err(|_| {
        "Enable Accessibility access for the herdr-handsfree executable in macOS System Settings"
            .to_string()
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{PushToTalk, Recording, Shared, SpaceAction, space_action};
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    #[test]
    fn space_is_captured_only_when_eligible_and_release_ends_even_after_focus_change() {
        assert_eq!(space_action(true, false, false), SpaceAction::Pass);
        assert_eq!(space_action(true, false, true), SpaceAction::Begin);
        assert_eq!(space_action(true, true, false), SpaceAction::SuppressRepeat);
        assert_eq!(space_action(false, true, false), SpaceAction::End);
        assert_eq!(space_action(false, false, true), SpaceAction::Pass);
    }

    #[test]
    fn only_audio_between_press_and_release_is_retained() {
        let mut r = Recording {
            enabled: false,
            samples: Vec::new(),
        };
        r.capture(&[0.1, 0.1], 2, 1.0);
        r.begin();
        r.capture(&[0.2, 0.4, 0.6, 0.8], 2, 1.0);
        let samples = r.finish();
        r.capture(&[0.9, 0.9], 2, 1.0);
        assert_eq!(samples.len(), 2);
        assert!((samples[0] - 0.3).abs() < 0.0001);
        assert!((samples[1] - 0.7).abs() < 0.0001);
        assert!(r.samples.is_empty());
    }

    #[test]
    fn stop_waits_for_recording_and_queued_delivery() {
        let shared = Arc::new(Shared::default());
        let mode = PushToTalk(shared.clone());
        shared.held.store(true, Ordering::SeqCst);
        assert!(!mode.try_stop());
        shared.pending.store(1, Ordering::SeqCst);
        shared.held.store(false, Ordering::SeqCst);
        assert!(!mode.try_stop());
        shared.pending.store(0, Ordering::SeqCst);
        assert!(mode.try_stop());
        assert!(shared.stop.load(Ordering::SeqCst));
    }

    #[test]
    #[ignore = "requires a local model and HANDSFREE_TEST_WAV; runs a fake Herdr command"]
    fn recorded_audio_is_transcribed_and_sent_to_its_original_pane() {
        use std::os::unix::fs::PermissionsExt;
        let wav = std::env::var("HANDSFREE_TEST_WAV").expect("HANDSFREE_TEST_WAV");
        let dir = std::env::temp_dir().join(format!("handsfree-ptt-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let command = dir.join("herdr");
        let output = dir.join("sent");
        std::fs::write(
            &command,
            format!(
                "#!/bin/sh\nprintf '%s|%s' \"$3\" \"$4\" > '{}'\n",
                output.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o700)).unwrap();
        unsafe {
            std::env::set_var("HERDR_BIN_PATH", &command);
        }
        let mut reader = hound::WavReader::open(wav).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        let samples = reader
            .samples::<i16>()
            .map(|x| x.unwrap() as f32 / 32768.0)
            .collect();
        let job = super::Job {
            pane: "original-pane".into(),
            samples,
            sample_rate: spec.sample_rate as usize,
        };
        super::transcribe_and_deliver(super::stt::Stt::shared().unwrap(), job).unwrap();
        let delivered = std::fs::read_to_string(&output).unwrap();
        assert!(delivered.starts_with("original-pane|"));
        assert!(delivered.len() > "original-pane|".len());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

fn record_loop(shared: &Arc<Shared>, rx: mpsc::Receiver<KeyEvent>) -> Result<(), String> {
    let stt = stt::Stt::shared().map_err(|e| format!("speech model: {e}"))?;
    let host = cpal::default_host();
    let device = dictation::pick_device(&host).ok_or("no input device")?;
    let config = device.default_input_config().map_err(|e| e.to_string())?;
    let sample_rate = config.sample_rate() as usize;
    let channels = config.channels() as usize;
    let stream_config: cpal::StreamConfig = config.into();
    let gain = crate::config::get().input_gain.max(0.0);
    let buffer = Arc::new(Mutex::new(Recording {
        enabled: false,
        samples: Vec::new(),
    }));
    let (jobs_tx, jobs_rx) = mpsc::channel::<Job>();
    let jobs_shared = shared.clone();
    std::thread::spawn(move || {
        while let Ok(job) = jobs_rx.recv() {
            if let Err(e) = transcribe_and_deliver(stt, job) {
                eprintln!("push-to-talk transcription or delivery failed: {e}");
                jobs_shared.error.store(true, Ordering::SeqCst);
            }
            jobs_shared.pending.fetch_sub(1, Ordering::SeqCst);
        }
    });
    shared.mic_ready.store(true, Ordering::SeqCst);
    let mut pane = None;
    let mut active_stream: Option<cpal::Stream> = None;
    while !shared.stop.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(KeyEvent::Press(id, sequence)) => {
                pane = Some(id);
                {
                    let mut record = buffer.lock().map_err(|e| e.to_string())?;
                    record.begin();
                }
                let audio_buffer = buffer.clone();
                let error_shared = shared.clone();
                let input_shared = shared.clone();
                let stream = device
                    .build_input_stream(
                        stream_config.clone(),
                        move |data: &[f32], _: &cpal::InputCallbackInfo| {
                            if input_shared.held.load(Ordering::SeqCst)
                                && input_shared.sequence.load(Ordering::SeqCst) == sequence
                                && let Ok(mut record) = audio_buffer.lock()
                            {
                                if input_shared.held.load(Ordering::SeqCst)
                                    && input_shared.sequence.load(Ordering::SeqCst) == sequence
                                {
                                    record.capture(data, channels, gain);
                                }
                            }
                        },
                        move |e| {
                            eprintln!("push-to-talk microphone error: {e}");
                            error_shared.error.store(true, Ordering::SeqCst);
                        },
                        None,
                    )
                    .map_err(|e| format!("microphone stream: {e}"))?;
                stream
                    .play()
                    .map_err(|e| format!("microphone start: {e}"))?;
                active_stream = Some(stream);
                shared.recording.store(true, Ordering::SeqCst);
            }
            Ok(KeyEvent::Release) => {
                let samples = buffer.lock().map_err(|e| e.to_string())?.finish();
                // Drop the stream so the microphone is closed while idle.
                drop(active_stream.take());
                shared.recording.store(false, Ordering::SeqCst);
                if let Some(id) = pane.take() {
                    if jobs_tx
                        .send(Job {
                            pane: id,
                            samples,
                            sample_rate,
                        })
                        .is_err()
                    {
                        shared.pending.fetch_sub(1, Ordering::SeqCst);
                        return Err("transcription worker stopped".into());
                    }
                } else {
                    shared.pending.fetch_sub(1, Ordering::SeqCst);
                }
            }
            Ok(KeyEvent::Cancel) => {
                {
                    let mut record = buffer.lock().map_err(|e| e.to_string())?;
                    record.enabled = false;
                    record.samples.clear();
                }
                drop(active_stream.take());
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) if shared.error.load(Ordering::SeqCst) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    shared.mic_ready.store(false, Ordering::SeqCst);
    Ok(())
}
