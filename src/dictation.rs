//! Dictation pipeline: mic (cpal) → energy VAD → STT → focused pane.

use crate::{stt::Stt, vad};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

#[derive(Clone)]
pub(crate) struct AudioChunk {
    samples: Vec<f32>,
    focus_epoch: u64,
    pane_id: Option<String>,
}

impl AudioChunk {
    pub(crate) fn captured(samples: Vec<f32>, tracker: &crate::deliver::FocusTracker) -> Self {
        let (focus_epoch, pane_id) = tracker.snapshot();
        Self {
            samples,
            focus_epoch,
            pane_id,
        }
    }
}

pub struct Dictation {
    stop: Arc<AtomicBool>,
    telemetry: Arc<Telemetry>,
}

#[derive(Default)]
pub(crate) struct Telemetry {
    capture_ready: AtomicBool,
    samples_received: AtomicBool,
    model_ready: AtomicBool,
    speech_detected: AtomicBool,
    settling: AtomicBool,
    processing: AtomicBool,
    checking: AtomicBool,
    waiting: AtomicBool,
    error: AtomicBool,
}

impl Telemetry {
    pub(crate) fn phase(&self) -> &'static str {
        if self.error.load(Ordering::SeqCst) {
            "error"
        } else if self.processing.load(Ordering::SeqCst) {
            "transcribing"
        } else if self.speech_detected.load(Ordering::SeqCst) {
            "speech"
        } else if self.settling.load(Ordering::SeqCst) {
            "settling"
        } else if self.checking.load(Ordering::SeqCst) {
            "checking"
        } else if self.waiting.load(Ordering::SeqCst) {
            "waiting"
        } else if self.capture_ready.load(Ordering::SeqCst)
            && self.samples_received.load(Ordering::SeqCst)
            && self.model_ready.load(Ordering::SeqCst)
        {
            "listening"
        } else {
            "preparing"
        }
    }
}

impl Dictation {
    /// Start mic capture + transcription. Returns after threads are spawned;
    /// model load happens on the worker thread so toggling stays instant.
    pub fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let telemetry = Arc::new(Telemetry::default());
        let focus_tracker = crate::deliver::watch_focus(stop.clone());
        let (audio_tx, audio_rx) = mpsc::channel::<AudioChunk>();

        // Capture thread: owns the cpal stream (not Send), forwards mono chunks.
        let stop_capture = stop.clone();
        let capture_telemetry = telemetry.clone();
        let capture_focus = focus_tracker.clone();
        std::thread::spawn(move || {
            if let Err(e) =
                capture_loop(&stop_capture, audio_tx, &capture_telemetry, &capture_focus)
            {
                capture_telemetry.error.store(true, Ordering::SeqCst);
                eprintln!("dictation capture error: {e}");
            }
        });

        // Worker thread: VAD + STT + delivery.
        let stop_worker = stop.clone();
        let worker_telemetry = telemetry.clone();
        std::thread::spawn(move || {
            worker_loop(&stop_worker, audio_rx, &worker_telemetry, &focus_tracker)
        });

        Self { stop, telemetry }
    }

    pub fn phase(&self) -> &'static str {
        self.telemetry.phase()
    }
}

impl Drop for Dictation {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Case-insensitive substring match, so `microphone = "airpods"` picks
/// "Ranolp's AirPods Pro".
fn mic_matches(description: &str, wanted: &str) -> bool {
    description
        .to_lowercase()
        .contains(&wanted.trim().to_lowercase())
}

/// Config `microphone` (or HANDSFREE_MIC) selects the input device; empty or
/// unmatched falls back to the system default.
pub(crate) fn pick_device(host: &cpal::Host) -> Option<cpal::Device> {
    let wanted =
        std::env::var("HANDSFREE_MIC").unwrap_or_else(|_| crate::config::get().microphone.clone());
    if wanted.trim().is_empty() {
        return host.default_input_device();
    }
    let mut seen = Vec::new();
    for device in host.input_devices().ok()? {
        let name = device
            .description()
            .map(|d| d.to_string())
            .unwrap_or_default();
        if mic_matches(&name, &wanted) {
            return Some(device);
        }
        seen.push(name);
    }
    eprintln!(
        "microphone \"{wanted}\" not found; using default. available: {}",
        seen.join(", ")
    );
    host.default_input_device()
}

fn capture_loop(
    stop: &AtomicBool,
    tx: mpsc::Sender<AudioChunk>,
    telemetry: &Arc<Telemetry>,
    focus_tracker: &Arc<crate::deliver::FocusTracker>,
) -> Result<(), String> {
    let host = cpal::default_host();
    let device = pick_device(&host)
        .ok_or("no input device — check macOS microphone permission for your terminal app")?;
    let config = device
        .default_input_config()
        .map_err(|e| format!("input config: {e}"))?;
    let sample_rate = config.sample_rate() as usize;
    let channels = config.channels() as usize;
    let mic_name = device
        .description()
        .map(|d| d.to_string())
        .unwrap_or_default();
    eprintln!("mic: {mic_name} @ {sample_rate} Hz, {channels}ch");
    // Tell the worker the sample rate via a header chunk.
    let _ = tx.send(AudioChunk::captured(
        vec![sample_rate as f32],
        focus_tracker,
    ));

    let error_telemetry = telemetry.clone();
    let input_telemetry = telemetry.clone();
    let input_focus = focus_tracker.clone();
    let gain = crate::config::get().input_gain.max(0.0);
    let stream = device
        .build_input_stream(
            config.into(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                input_telemetry
                    .samples_received
                    .store(true, Ordering::SeqCst);
                // Downmix interleaved channels to mono.
                let mono: Vec<f32> = data
                    .chunks(channels)
                    .map(|frame| {
                        (frame.iter().sum::<f32>() / channels as f32 * gain).clamp(-1.0, 1.0)
                    })
                    .collect();
                let _ = tx.send(AudioChunk::captured(mono, &input_focus));
            },
            move |e| {
                error_telemetry.error.store(true, Ordering::SeqCst);
                eprintln!("mic stream error: {e}");
            },
            None,
        )
        .map_err(|e| format!("build input stream: {e}"))?;
    stream.play().map_err(|e| format!("stream play: {e}"))?;
    telemetry.capture_ready.store(true, Ordering::SeqCst);

    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    drop(stream);
    eprintln!("mic capture stopped");
    Ok(())
}

fn remember_target<T>(
    target: &mut Option<Result<T, String>>,
    was_in_speech: bool,
    in_speech: bool,
    lookup: impl FnOnce() -> Result<T, String>,
) {
    if !was_in_speech && in_speech && target.is_none() {
        *target = Some(lookup());
    }
}

fn should_submit(
    current_revision: u64,
    checked_revision: u64,
    speaking: bool,
    focus_unchanged: bool,
    complete: bool,
    cancelled: bool,
) -> bool {
    current_revision == checked_revision && !speaking && focus_unchanged && complete && !cancelled
}

fn needs_recheck(draft: &str, speaking: bool, checking: bool, waiting: bool) -> bool {
    !draft.is_empty() && !speaking && !checking && !waiting
}

const MAX_DRAFT_BYTES: usize = 8_192;

/// Focus moved away after this draft was transcribed: leaving the pane counts
/// as finishing the turn, so send it to the pane it was spoken in.
fn hand_off(draft: &mut String, target: &Option<Result<crate::deliver::FocusedPane, String>>) {
    if draft.is_empty() {
        return;
    }
    let text = std::mem::take(draft);
    match target {
        Some(Ok(pane)) => match crate::deliver::deliver_to_original(pane, &text) {
            Ok(()) => eprintln!(
                "[{}] focus moved; delivered to original {}: {:?}",
                crate::stamp(),
                pane.pane_id,
                text
            ),
            Err(e) => eprintln!("[{}] focus moved; delivery failed: {e}", crate::stamp()),
        },
        _ => eprintln!(
            "[{}] focus moved; no agent pane, discarded: {:?}",
            crate::stamp(),
            text
        ),
    }
}

/// An unsent draft older than this (since the last speech) is sent as is to
/// the pane it was spoken in, so a turn the judge missed is never lost and is
/// never glued onto unrelated later speech. With an end word configured, the
/// user gets time to notice a missed turn and say it.
const DRAFT_TTL: std::time::Duration = std::time::Duration::from_secs(15);
const END_WORD_DRAFT_TTL: std::time::Duration = std::time::Duration::from_secs(60);

fn draft_expired(
    draft: &str,
    idle: std::time::Duration,
    ttl: std::time::Duration,
    speaking: bool,
    checking: bool,
) -> bool {
    !draft.is_empty() && !speaking && !checking && idle >= ttl
}

fn trim_punct(text: &str) -> &str {
    text.trim_end_matches(|c: char| c.is_whitespace() || ".?!。？！,，~…".contains(c))
}

/// When the draft ends with the end word, return the text to submit without it.
fn strip_end_word<'a>(draft: &'a str, end_word: &str) -> Option<&'a str> {
    let body = trim_punct(draft).strip_suffix(end_word)?;
    Some(trim_punct(body))
}

/// (RMS, peak) of an utterance, logged to judge microphone gain.
fn levels(samples: &[f32]) -> (f32, f32) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    (rms, peak)
}

fn append_segment(draft: &mut String, segment: &str) -> bool {
    let segment = segment.trim();
    if segment.is_empty() {
        return true;
    }
    let separator_len = usize::from(!draft.is_empty());
    if draft
        .len()
        .saturating_add(separator_len)
        .saturating_add(segment.len())
        > MAX_DRAFT_BYTES
    {
        // Do not leave a permanently unsubmitable prefix behind. The caller
        // drops its pane binding and can start a fresh request on later speech.
        draft.clear();
        return false;
    }
    if !draft.is_empty() {
        draft.push(' ');
    }
    draft.push_str(segment);
    true
}

pub(crate) fn worker_loop(
    stop: &AtomicBool,
    rx: mpsc::Receiver<AudioChunk>,
    telemetry: &Telemetry,
    focus_tracker: &crate::deliver::FocusTracker,
) {
    // First chunk is the sample-rate header from the capture thread.
    let sample_rate = match rx.recv() {
        Ok(header) if header.samples.len() == 1 => header.samples[0] as usize,
        _ => {
            if !stop.load(Ordering::SeqCst) {
                telemetry.error.store(true, Ordering::SeqCst);
            }
            return;
        }
    };
    if stop.load(Ordering::SeqCst) {
        return;
    }
    let stt = match Stt::for_session() {
        Ok(s) => s,
        Err(e) => {
            telemetry.error.store(true, Ordering::SeqCst);
            eprintln!("dictation disabled: {e}");
            return;
        }
    };
    // Loading is synchronous: release immediately if switched off during it.
    if stop.load(Ordering::SeqCst) {
        return;
    }
    eprintln!("STT ready; listening");

    let mut resampler = vad::Resampler::new(sample_rate);
    let mut vad = vad::Vad::new();
    let mut capture_epoch = None;
    telemetry.model_ready.store(true, Ordering::SeqCst);
    // The utterance in progress is decoded as it arrives, so the wait after
    // end-of-speech is the VAD hangover rather than hangover + whole-utterance
    // inference. `fed` mirrors the VAD's emitted mark, so the tail between the
    // last drain and completion can still be handed over before finishing.
    let mut stream: Option<crate::stt::Stream<'_>> = None;
    let mut fed = 0usize;
    let mut target: Option<Result<crate::deliver::FocusedPane, String>> = None;
    let mut target_epoch: Option<u64> = None;
    // Set when streaming broke partway through the current utterance: the live
    // stream is then missing audio, so the utterance is batched at completion.
    let mut fallback = false;
    let mut draft = String::new();
    let mut revision = 0u64;
    let mut last_speech = std::time::Instant::now();
    let mut judge_retried = false;
    let end_word = crate::config::get().end_word.trim().to_string();
    let draft_ttl = if end_word.is_empty() {
        DRAFT_TTL
    } else {
        END_WORD_DRAFT_TTL
    };
    let (judge_tx, judge_rx) = mpsc::channel::<(u64, Result<bool, String>)>();
    let mut deferred_chunk = None;
    while !stop.load(Ordering::SeqCst) {
        // Keep consuming audio while the local language model judges a previous
        // segment. Process queued audio before any verdict, so resumed speech
        // invalidates a stale "complete" decision.
        let first = match deferred_chunk.take() {
            Some(c) => Some(c),
            None => match rx.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(c) => Some(c),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    // A capture device can stop after its last old-pane
                    // chunk. There may be no newer epoch to trigger handoff.
                    if !stop.load(Ordering::SeqCst) {
                        hand_off(&mut draft, &target);
                    }
                    break;
                }
            },
        };
        let mut chunks: std::collections::VecDeque<AudioChunk> = first.into_iter().collect();
        chunks.extend(rx.try_iter());
        while let Some(chunk) = chunks.pop_front() {
            // Replay captured epochs in FIFO order. Never throw away an active
            // utterance just because the live focus watcher has moved ahead.
            if !vad.in_speech() && capture_epoch != Some(chunk.focus_epoch) {
                capture_epoch = Some(chunk.focus_epoch);
                vad.reset();
                resampler = vad::Resampler::new(sample_rate);
                stream = None;
                fed = 0;
                fallback = false;
                // No utterance is active here; hand off the buffered draft.
                hand_off(&mut draft, &target);
                target = None;
                target_epoch = None;
                revision = revision.wrapping_add(1);
                telemetry.speech_detected.store(false, Ordering::SeqCst);
                telemetry.processing.store(false, Ordering::SeqCst);
                telemetry.checking.store(false, Ordering::SeqCst);
                telemetry.waiting.store(false, Ordering::SeqCst);
            }

            let was_in_speech = vad.in_speech();
            let done = if was_in_speech && !vad.voiced() && target_epoch != Some(chunk.focus_epoch)
            {
                // Silence followed by a different capture pane is an explicit
                // handoff. Finish the old audio before feeding ANY new-pane
                // samples; retry this chunk afterward, without losing it.
                chunks.push_front(chunk.clone());
                let utterance = vad.finish();
                vad.reset();
                utterance
            } else {
                vad.push(&resampler.push(&chunk.samples))
            };
            if !was_in_speech && vad.in_speech() {
                revision = revision.wrapping_add(1);
                let current = chunk
                    .pane_id
                    .as_ref()
                    .map(|id| crate::deliver::FocusedPane {
                        pane_id: id.clone(),
                    })
                    .ok_or_else(|| "no agent pane at capture time".to_string());
                let same = match (&target, &current) {
                    (Some(Ok(old)), Ok(now)) => {
                        old.pane_id == now.pane_id && target_epoch == Some(chunk.focus_epoch)
                    }
                    (None, _) => true,
                    _ => false,
                };
                if !same {
                    hand_off(&mut draft, &target);
                    target = None;
                    target_epoch = None;
                }
                remember_target(&mut target, false, true, || current);
                last_speech = std::time::Instant::now();
                if target_epoch.is_none() {
                    target_epoch = Some(chunk.focus_epoch);
                }
                telemetry.checking.store(false, Ordering::SeqCst);
                telemetry.waiting.store(false, Ordering::SeqCst);
            }
            telemetry
                .speech_detected
                .store(vad.voiced(), Ordering::SeqCst);
            telemetry
                .settling
                .store(vad.in_speech() && !vad.voiced(), Ordering::SeqCst);

            if let Some(utterance) = done {
                telemetry.processing.store(true, Ordering::SeqCst);
                let (rms, peak) = levels(&utterance);
                let text = match stream.take() {
                    Some(mut s) if !fallback => s
                        .feed(&utterance[fed.min(utterance.len())..])
                        .and_then(|()| s.finish()),
                    _ => stt.transcribe(&utterance),
                };
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                fed = 0;
                fallback = false;
                let text = text.map(|text| crate::config::correct_transcript(&text));
                match text {
                    Ok(text) if !text.trim().is_empty() => {
                        if target_epoch == Some(focus_tracker.epoch())
                            && target
                                .as_ref()
                                .and_then(|t| t.as_ref().ok())
                                .is_some_and(crate::deliver::same_as_current)
                        {
                            if !append_segment(&mut draft, &text) {
                                eprintln!("spoken draft exceeded the safety limit; discarded");
                                target = None;
                                target_epoch = None;
                                revision = revision.wrapping_add(1);
                                telemetry.waiting.store(false, Ordering::SeqCst);
                                telemetry.checking.store(false, Ordering::SeqCst);
                                telemetry.processing.store(false, Ordering::SeqCst);
                                continue;
                            }
                            eprintln!(
                                "[{}] segment transcribed (rms {rms:.3}, peak {peak:.2}): {:?}",
                                crate::stamp(),
                                text.trim()
                            );
                            last_speech = std::time::Instant::now();
                            judge_retried = false;
                            // The end word is a manual override for when the
                            // remote judge misses a finished turn; otherwise
                            // the judge below decides.
                            let manual = if end_word.is_empty() {
                                None
                            } else {
                                strip_end_word(&draft, &end_word).map(str::to_string)
                            };
                            if let Some(body) = manual {
                                revision = revision.wrapping_add(1);
                                draft.clear();
                                if body.is_empty() {
                                    eprintln!("[{}] end word with nothing to send", crate::stamp());
                                    target = None;
                                    target_epoch = None;
                                } else {
                                    let pane = target.take().unwrap().unwrap();
                                    target_epoch = None;
                                    // Focus may move after the check above;
                                    // the captured destination still owns it.
                                    match crate::deliver::deliver_to_original(&pane, &body) {
                                        Ok(()) => eprintln!(
                                            "[{}] end word; delivered to {}: {:?}",
                                            crate::stamp(),
                                            pane.pane_id,
                                            body
                                        ),
                                        Err(e) => {
                                            eprintln!("[{}] delivery failed: {e}", crate::stamp())
                                        }
                                    }
                                }
                                telemetry.checking.store(false, Ordering::SeqCst);
                                telemetry.waiting.store(false, Ordering::SeqCst);
                                telemetry.processing.store(false, Ordering::SeqCst);
                                continue;
                            }
                            revision = revision.wrapping_add(1);
                            let checked = revision;
                            let snapshot = draft.clone();
                            let sender = judge_tx.clone();
                            telemetry.checking.store(true, Ordering::SeqCst);
                            std::thread::spawn(move || {
                                let _ = sender.send((checked, crate::turn_gate::judge(&snapshot)));
                            });
                        } else {
                            // Capture is queued while transcription runs. A
                            // focus change can happen after more old-pane
                            // audio was already captured, so wait until that
                            // queue reaches the new epoch before handing off.
                            if !append_segment(&mut draft, &text) {
                                draft.clear();
                            }
                            revision = revision.wrapping_add(1);
                            telemetry.checking.store(false, Ordering::SeqCst);
                            telemetry.waiting.store(false, Ordering::SeqCst);
                        }
                    }
                    Ok(_) => {}
                    Err(e) => eprintln!("transcription failed: {e}"),
                }
                telemetry.processing.store(false, Ordering::SeqCst);
            } else if !vad.in_speech() {
                // Speech shorter than MIN_SPEECH_MS is not submitted.
                stream = None;
                fed = 0;
                fallback = false;
                if draft.is_empty() {
                    target = None;
                    target_epoch = None;
                }
            }

            if vad.in_speech() && !fallback {
                let pending = vad.drain_speech();
                if !pending.is_empty() {
                    let s = stream.get_or_insert_with(|| stt.stream());
                    if let Err(e) = s.feed(&pending) {
                        eprintln!("streaming transcription failed ({e}); batching this utterance");
                        fallback = true;
                    }
                    fed += pending.len();
                }
            }
        }

        // Whisper may decode for seconds while new audio queues. Drain it
        // before accepting a potentially stale model completion verdict.
        if stop.load(Ordering::SeqCst) {
            return;
        }
        if let Ok(chunk) = rx.try_recv() {
            deferred_chunk = Some(chunk);
            continue;
        }
        while let Ok((checked, result)) = judge_rx.try_recv() {
            if checked != revision {
                continue;
            }
            telemetry.checking.store(false, Ordering::SeqCst);
            let current_target = target.as_ref().and_then(|t| t.as_ref().ok());
            let focus_unchanged = target_epoch == Some(focus_tracker.epoch())
                && current_target.is_some_and(crate::deliver::same_as_current);
            if !focus_unchanged {
                // The live focus watcher can move ahead of audio that is
                // already queued. The capture epoch transition hands off
                // only after that older audio has been processed.
                revision = revision.wrapping_add(1);
                telemetry.waiting.store(false, Ordering::SeqCst);
                continue;
            }
            match result {
                Ok(complete)
                    if should_submit(
                        revision,
                        checked,
                        vad.in_speech(),
                        true,
                        complete,
                        stop.load(Ordering::SeqCst),
                    ) =>
                {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    let text = std::mem::take(&mut draft);
                    let pane = target.take().unwrap().unwrap();
                    target_epoch = None;
                    if let Err(e) = crate::deliver::deliver_to_original(&pane, &text) {
                        eprintln!("[{}] delivery failed: {e}", crate::stamp());
                    } else {
                        eprintln!(
                            "[{}] complete; delivered to {}: {:?}",
                            crate::stamp(),
                            pane.pane_id,
                            text
                        );
                    }
                    revision = revision.wrapping_add(1);
                    telemetry.waiting.store(false, Ordering::SeqCst);
                }
                Ok(complete) => {
                    eprintln!(
                        "[{}] not submitted (complete={complete}, speaking={}); waiting: {:?}",
                        crate::stamp(),
                        vad.in_speech(),
                        draft
                    );
                    telemetry.waiting.store(!draft.is_empty(), Ordering::SeqCst);
                }
                Err(e) => {
                    eprintln!(
                        "[{}] semantic check failed: {e}; {}",
                        crate::stamp(),
                        if judge_retried {
                            "no automatic submission"
                        } else {
                            "retrying once"
                        }
                    );
                    // Leaving `waiting` false lets the recheck below retry once.
                    telemetry
                        .waiting
                        .store(judge_retried && !draft.is_empty(), Ordering::SeqCst);
                    judge_retried = true;
                }
            }
        }
        if draft_expired(
            &draft,
            last_speech.elapsed(),
            draft_ttl,
            vad.in_speech(),
            telemetry.checking.load(Ordering::SeqCst),
        ) {
            let text = std::mem::take(&mut draft);
            match &target {
                Some(Ok(pane)) => match crate::deliver::deliver_to_original(pane, &text) {
                    Ok(()) => eprintln!(
                        "[{}] unsent draft expired after {}s of silence; delivered to {}: {:?}",
                        crate::stamp(),
                        draft_ttl.as_secs(),
                        pane.pane_id,
                        text
                    ),
                    Err(e) => eprintln!(
                        "[{}] unsent draft expired; delivery failed: {e}",
                        crate::stamp()
                    ),
                },
                _ => eprintln!(
                    "[{}] unsent draft expired; no agent pane, discarded: {:?}",
                    crate::stamp(),
                    text
                ),
            }
            target = None;
            target_epoch = None;
            revision = revision.wrapping_add(1);
            telemetry.waiting.store(false, Ordering::SeqCst);
        }
        // A brief noise burst can invalidate an in-flight verdict without
        // producing a segment. When quiet returns, recheck the existing draft
        // instead of leaving it invisible in the listening state forever.
        if needs_recheck(
            &draft,
            vad.in_speech(),
            telemetry.checking.load(Ordering::SeqCst),
            telemetry.waiting.load(Ordering::SeqCst),
        ) {
            let same_focus = target_epoch == Some(focus_tracker.epoch())
                && target
                    .as_ref()
                    .and_then(|t| t.as_ref().ok())
                    .is_some_and(crate::deliver::same_as_current);
            if same_focus {
                let snapshot = draft.clone();
                let checked = revision;
                let sender = judge_tx.clone();
                telemetry.checking.store(true, Ordering::SeqCst);
                eprintln!(
                    "rechecking buffered turn after brief speech ({} chars)",
                    draft.chars().count()
                );
                std::thread::spawn(move || {
                    let _ = sender.send((checked, crate::turn_gate::judge(&snapshot)));
                });
            } else {
                // Wait for the captured epoch transition: the live watcher
                // may be ahead of old-pane audio already in the queue.
            }
        }
    }
    // Toggle-off cancels pending, unsent speech. Never deliver from Drop.
}

#[cfg(test)]
mod tests {
    use super::{Telemetry, mic_matches};
    use std::sync::atomic::Ordering;

    #[test]
    #[ignore = "requires macOS say, local Whisper, and user-provided Jev API credit"]
    #[cfg(target_os = "macos")]
    fn brief_voice_does_not_strand_a_completed_turn_in_production_worker() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::{Arc, atomic::AtomicBool, mpsc};
        let dir = std::env::temp_dir().join(format!("handsfree-recheck-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("speech.wav");
        assert!(
            std::process::Command::new("say")
                .args(["-v", "Yuna", "-o"])
                .arg(&wav)
                .args(["--data-format=LEI16@16000", "이 설정을 고쳐줘."])
                .status()
                .unwrap()
                .success()
        );
        let script = dir.join("mock-herdr");
        let sent = dir.join("sent");
        std::fs::write(&script, format!(
            "#!/bin/sh\nif [ \"$1 $2\" = 'pane list' ]; then printf '{{\"result\":{{\"panes\":[{{\"pane_id\":\"pane-A\",\"focused\":true,\"agent\":\"hermes\"}}]}}}}'; elif [ \"$1 $2\" = 'agent prompt' ]; then printf '%s' \"$4\" > '{}'; else exit 2; fi\n", sent.display()
        )).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        unsafe {
            std::env::set_var("HERDR_BIN_PATH", &script);
            std::env::set_var(
                "HERDR_PLUGIN_CONFIG_DIR",
                std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
                    .join(".config/herdr/plugins/config/ranolp.handsfree"),
            );
            std::env::set_var("HANDSFREE_VAD", "energy");
            std::env::set_var("HANDSFREE_STT_CPU", "1");
        }
        let stop = Arc::new(AtomicBool::new(false));
        let telemetry = Arc::new(Telemetry::default());
        let tracker = crate::deliver::watch_focus(stop.clone());
        let (tx, rx) = mpsc::channel();
        let stopped = stop.clone();
        let observed = telemetry.clone();
        let tracked = tracker.clone();
        let worker =
            std::thread::spawn(move || super::worker_loop(&stopped, rx, &observed, &tracked));
        tx.send(super::AudioChunk::captured(vec![16_000.0], &tracker))
            .unwrap();
        let audio: Vec<f32> = hound::WavReader::open(&wav)
            .unwrap()
            .samples::<i16>()
            .map(|s| s.unwrap() as f32 / 32768.0)
            .collect();
        for chunk in audio.chunks(512) {
            tx.send(super::AudioChunk::captured(chunk.to_vec(), &tracker))
                .unwrap();
        }
        for _ in 0..100 {
            tx.send(super::AudioChunk::captured(vec![0.0; 512], &tracker))
                .unwrap();
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(40);
        while !telemetry.checking.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            telemetry.checking.load(Ordering::SeqCst),
            "synthetic speech was not transcribed and judged"
        );
        // <250ms speech burst invalidates the first verdict, but produces no
        // second transcript. A fresh verdict must run when quiet returns.
        for _ in 0..4 {
            tx.send(super::AudioChunk::captured(vec![0.5; 512], &tracker))
                .unwrap();
        }
        for _ in 0..100 {
            tx.send(super::AudioChunk::captured(vec![0.0; 512], &tracker))
                .unwrap();
        }
        while !sent.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        stop.store(true, Ordering::SeqCst);
        drop(tx);
        worker.join().unwrap();
        assert!(
            sent.exists(),
            "completed buffered speech was stranded after a brief burst"
        );
        assert!(std::fs::read_to_string(&sent).unwrap().contains("고쳐줘"));
        unsafe {
            std::env::remove_var("HERDR_BIN_PATH");
            std::env::remove_var("HERDR_PLUGIN_CONFIG_DIR");
            std::env::remove_var("HANDSFREE_VAD");
            std::env::remove_var("HANDSFREE_STT_CPU");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[ignore = "requires macOS say and a local Whisper model; run explicitly with --ignored"]
    #[cfg(target_os = "macos")]
    fn synthesized_speech_crossing_panes_reaches_only_the_original_agent() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::{Arc, atomic::AtomicBool, mpsc};
        let dir = std::env::temp_dir().join(format!("handsfree-safety-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("speech.wav");
        assert!(
            std::process::Command::new("say")
                .args(["-v", "Yuna", "-o"])
                .arg(&wav)
                .args([
                    "--data-format=LEI16@16000",
                    "다른 페인에 이전 말을 보내지 마세요."
                ])
                .status()
                .unwrap()
                .success()
        );
        let script = dir.join("mock-herdr");
        let focus = dir.join("focus");
        let sent = dir.join("sent");
        std::fs::write(&script, format!(
            "#!/bin/sh\nif [ \"$1 $2\" = 'pane list' ]; then printf '{{\"result\":{{\"panes\":[{{\"pane_id\":\"%s\",\"focused\":true,\"agent\":\"hermes\"}}]}}}}' \"$(/bin/cat '{}')\"; elif [ \"$1 $2\" = 'agent prompt' ]; then printf '%s|%s\\n' \"$3\" \"$4\" >> '{}'; else exit 2; fi\n",
            focus.display(), sent.display()
        )).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(&focus, "pane-A").unwrap();
        std::fs::write(
            dir.join("config.toml"),
            "language = \"ko\"\nvad = \"energy\"\nmin_speech_rms = 0.0\n",
        )
        .unwrap();
        unsafe {
            std::env::set_var("HERDR_BIN_PATH", &script);
            std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", &dir);
            std::env::set_var(
                "HERDR_PLUGIN_STATE_DIR",
                std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
                    .join(".local/state/herdr/plugins/ranolp.handsfree"),
            );
            std::env::set_var("HANDSFREE_STT_CPU", "1");
        }
        let stop = Arc::new(AtomicBool::new(false));
        let telemetry = Arc::new(Telemetry::default());
        let tracker = crate::deliver::watch_focus(stop.clone());
        let (tx, rx) = mpsc::channel();
        let stopped = stop.clone();
        let observed = telemetry.clone();
        let tracked = tracker.clone();
        let worker =
            std::thread::spawn(move || super::worker_loop(&stopped, rx, &observed, &tracked));
        tx.send(super::AudioChunk::captured(vec![16_000.0], &tracker))
            .unwrap();
        let audio: Vec<f32> = hound::WavReader::open(&wav)
            .unwrap()
            .samples::<i16>()
            .map(|s| s.unwrap() as f32 / 32768.0)
            .collect();
        for chunk in audio.chunks(512) {
            tx.send(super::AudioChunk::captured(chunk.to_vec(), &tracker))
                .unwrap();
        }
        let mut detected = false;
        // First-run Metal/Whisper initialization can exceed ten seconds.
        for _ in 0..1_000 {
            if telemetry.speech_detected.load(Ordering::SeqCst) {
                detected = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            detected,
            "synthetic voice was not detected by the actual VAD"
        );
        let previous_epoch = tracker.epoch();
        std::fs::write(&focus, "pane-B").unwrap();
        for _ in 0..100 {
            if tracker.epoch() != previous_epoch {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_ne!(
            tracker.epoch(),
            previous_epoch,
            "production focus watcher did not observe pane-B"
        );
        for _ in 0..190 {
            tx.send(super::AudioChunk::captured(vec![0.0; 512], &tracker))
                .unwrap();
        }
        drop(tx);
        worker.join().unwrap();
        stop.store(true, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(100));
        let delivered = std::fs::read_to_string(&sent).unwrap_or_default();
        assert!(
            delivered.starts_with("pane-A|"),
            "speech was lost: {delivered:?}"
        );
        assert!(
            !delivered.contains("pane-B|"),
            "speech from pane-A reached pane-B"
        );
        unsafe {
            std::env::remove_var("HERDR_BIN_PATH");
            std::env::remove_var("HERDR_PLUGIN_CONFIG_DIR");
            std::env::remove_var("HERDR_PLUGIN_STATE_DIR");
            std::env::remove_var("HANDSFREE_STT_CPU");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn queued_audio_retains_a_consistent_capture_pane_and_epoch() {
        let tracker = crate::deliver::FocusTracker::new(Some("pane-A".into()));
        let captured_epoch = tracker.epoch();
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(super::AudioChunk::captured(vec![0.25], &tracker))
            .unwrap();
        tracker.observe(Some("pane-B".into()));

        let queued = rx.recv().unwrap();
        assert_eq!(queued.samples, vec![0.25]);
        assert_eq!(queued.focus_epoch, captured_epoch);
        assert_eq!(queued.pane_id.as_deref(), Some("pane-A"));

        let new_focus_audio = super::AudioChunk::captured(vec![0.5], &tracker);
        assert_ne!(new_focus_audio.focus_epoch, captured_epoch);
        assert_eq!(new_focus_audio.pane_id.as_deref(), Some("pane-B"));
    }

    #[test]
    fn cancellation_vetoes_an_otherwise_complete_turn() {
        assert!(!super::should_submit(1, 1, false, true, true, true));
    }

    #[test]
    fn later_speech_invalidates_an_earlier_semantic_yes() {
        assert!(!super::should_submit(2, 1, false, true, true, false));
        assert!(!super::should_submit(1, 1, true, true, true, false));
        assert!(!super::should_submit(1, 1, false, false, true, false));
        assert!(!super::should_submit(1, 1, false, true, false, false));
        assert!(super::should_submit(1, 1, false, true, true, false));
    }

    #[test]
    fn brief_voice_after_a_judge_request_cannot_strand_an_unsent_draft() {
        // Speech onset invalidates the old verdict. If that noise fails the
        // minimum segment length, no new transcription arrives to restart the
        // judge; the buffered draft still needs a fresh check at silence.
        assert!(super::needs_recheck("끝난 요청", false, false, false));
        assert!(!super::needs_recheck("끝난 요청", true, false, false));
        assert!(!super::needs_recheck("끝난 요청", false, true, false));
        assert!(!super::needs_recheck("끝난 요청", false, false, true));
        assert!(!super::needs_recheck("", false, false, false));
    }

    #[test]
    fn unsent_draft_expires_only_after_quiet_ttl() {
        let ttl = super::DRAFT_TTL;
        let short = std::time::Duration::from_secs(1);
        assert!(super::draft_expired("하다가", ttl, ttl, false, false));
        assert!(!super::draft_expired("하다가", short, ttl, false, false));
        assert!(!super::draft_expired("하다가", ttl, ttl, true, false));
        assert!(!super::draft_expired("하다가", ttl, ttl, false, true));
        assert!(!super::draft_expired("", ttl, ttl, false, false));
    }

    #[test]
    fn end_word_submits_the_text_before_it() {
        assert_eq!(
            super::strip_end_word("설정 고쳐줘 이상.", "이상"),
            Some("설정 고쳐줘")
        );
        assert_eq!(
            super::strip_end_word("설정 고쳐줘. 이상", "이상"),
            Some("설정 고쳐줘")
        );
        assert_eq!(super::strip_end_word("이상", "이상"), Some(""));
        assert_eq!(super::strip_end_word("화면이 이상해", "이상"), None);
        assert_eq!(super::strip_end_word("계속 말하는 중", "이상"), None);
    }

    #[test]
    fn oversized_draft_is_discarded_and_next_request_can_start() {
        let mut text = String::new();
        assert!(super::append_segment(&mut text, &"x".repeat(8_192)));
        assert!(!super::append_segment(&mut text, "y"));
        assert!(text.is_empty());
        assert!(super::append_segment(&mut text, "new request"));
        assert_eq!(text, "new request");
    }

    #[test]
    fn incomplete_segments_remain_one_request() {
        let mut text = String::new();
        super::append_segment(&mut text, "내가 말하다가");
        super::append_segment(&mut text, "멈춰도 끊기지 않게 해줘");
        assert_eq!(text, "내가 말하다가 멈춰도 끊기지 않게 해줘");
    }

    #[test]
    fn incomplete_korean_clause_blocks_premature_model_yes() {
        assert!(!crate::turn_gate::safe_verdict(
            "다른 페인으로 넘어가면?",
            true
        ));
        assert!(!crate::turn_gate::safe_verdict("근데 한 가지는?", true));
        assert!(crate::turn_gate::safe_verdict("이 설정을 바꿔줘.", true));
    }

    #[test]
    fn malformed_or_failed_jev_response_never_submits() {
        assert_eq!(
            crate::turn_gate::parse_jev_verdict(
                br#"{"answers":{"complete":{"type":"noul","noul":0.91}}}"#
            ),
            Some(true)
        );
        assert_eq!(
            crate::turn_gate::parse_jev_verdict(
                br#"{"answers":{"complete":{"type":"noul","noul":0.07}}}"#
            ),
            Some(false)
        );
        assert_eq!(crate::turn_gate::parse_jev_verdict(b"offline"), None);
    }

    #[test]
    fn destination_stays_bound_to_speech_start_across_focus_change() {
        let mut destination = None;
        super::remember_target(&mut destination, false, true, || Ok("pane-A"));
        super::remember_target(&mut destination, true, true, || Ok("pane-B"));
        assert_eq!(destination.take().unwrap().unwrap(), "pane-A");
        super::remember_target(&mut destination, false, true, || Ok("pane-B"));
        assert_eq!(destination.take().unwrap().unwrap(), "pane-B");
    }

    #[test]
    fn phases_report_actual_readiness_and_failure() {
        let t = Telemetry::default();
        assert_eq!(t.phase(), "preparing");
        t.model_ready.store(true, Ordering::SeqCst);
        assert_eq!(t.phase(), "preparing");
        t.capture_ready.store(true, Ordering::SeqCst);
        assert_eq!(t.phase(), "preparing");
        t.samples_received.store(true, Ordering::SeqCst);
        assert_eq!(t.phase(), "listening");
        t.speech_detected.store(true, Ordering::SeqCst);
        assert_eq!(t.phase(), "speech");
        t.processing.store(true, Ordering::SeqCst);
        assert_eq!(t.phase(), "transcribing");
        t.error.store(true, Ordering::SeqCst);
        assert_eq!(t.phase(), "error");
    }

    #[test]
    fn matches_case_insensitive_substring() {
        assert!(mic_matches("Ranolp's AirPods Pro", " airpods "));
        assert!(!mic_matches("MacBook Pro Microphone", "airpods"));
    }
}
