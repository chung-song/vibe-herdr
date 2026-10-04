//! Whisper transcription (whisper.cpp via whisper-rs, Metal on macOS).

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub struct Stt {
    ctx: WhisperContext,
}

impl Stt {
    /// Load the ggml model (downloads on first run).
    pub fn load() -> std::io::Result<Self> {
        // whisper.cpp otherwise writes decoded token fragments to stderr,
        // which the daemon redirects into a persistent log. No logging backend
        // is enabled for whisper-rs, so this suppresses those private tokens.
        whisper_rs::install_logging_hooks();
        let model = crate::models::ensure_whisper_model()?;
        let mut params = WhisperContextParameters::default();
        // Isolated safety tests avoid whisper.cpp Metal's process-exit
        // residency-set assertion; normal dictation still uses the GPU.
        if std::env::var_os("HANDSFREE_STT_CPU").is_some() {
            params.use_gpu(false);
        }
        let ctx =
            WhisperContext::new_with_params(model.to_str().expect("model path is utf-8"), params)
                .map_err(|e| std::io::Error::other(format!("whisper load failed: {e}")))?;
        Ok(Self { ctx })
    }

    /// Transcribe mono 16 kHz f32 samples; returns trimmed text ("" for silence).
    pub fn transcribe(&self, samples_16k: &[f32]) -> std::io::Result<String> {
        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| std::io::Error::other(format!("whisper state: {e}")))?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        let language = &crate::config::get().language;
        params.set_language(if language == "auto" {
            None
        } else {
            Some(language)
        });
        let prompt = crate::config::whisper_glossary()?;
        if !prompt.is_empty() {
            params.set_initial_prompt(&prompt);
        }
        params.set_translate(false);
        params.set_no_context(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        state
            .full(params, samples_16k)
            .map_err(|e| std::io::Error::other(format!("whisper full: {e}")))?;

        let mut text = String::new();
        for i in 0..state.full_n_segments() {
            if let Some(seg) = state.get_segment(i) {
                if let Ok(s) = seg.to_str_lossy() {
                    text.push_str(&s);
                }
            }
        }
        Ok(text.trim().to_string())
    }

    pub fn stream(&self) -> Stream<'_> {
        Stream {
            stt: self,
            samples: Vec::new(),
        }
    }
}

/// whisper.cpp has no incremental decode here, so this only satisfies the seam
/// `dictation.rs` speaks: buffer everything, transcribe once at the end.
pub struct Stream<'a> {
    stt: &'a Stt,
    samples: Vec<f32>,
}

impl Stream<'_> {
    pub fn feed(&mut self, samples_16k: &[f32]) -> std::io::Result<()> {
        self.samples.extend_from_slice(samples_16k);
        Ok(())
    }

    pub fn finish(self) -> std::io::Result<String> {
        self.stt.transcribe(&self.samples)
    }
}
