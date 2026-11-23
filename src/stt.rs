//! Speech-to-text. The default backend is Qwen3-ASR on candle (pure Rust,
//! Metal on macOS); `--features whisper` swaps in whisper.cpp, which needs
//! cmake and a C++ toolchain.

#[cfg(not(feature = "whisper"))]
mod qwen;
#[cfg(feature = "whisper")]
mod whisper;

#[cfg(not(feature = "whisper"))]
pub use qwen::{Stream, Stt};
#[cfg(feature = "whisper")]
pub use whisper::{Stream, Stt};

impl Stt {
    /// The active dictation worker owns its model. Once dictation is turned
    /// off and the worker finishes, dropping it releases the model as well.
    /// The next session reloads it; failed loads can also be retried.
    pub fn for_session() -> std::io::Result<Self> {
        let t0 = std::time::Instant::now();
        let loaded = Self::load();
        eprintln!("STT model load finished in {:.1?}", t0.elapsed());
        loaded
    }
}

#[cfg(all(test, feature = "whisper"))]
mod tests {
    #[test]
    #[ignore = "loads the configured local Whisper model twice; no mic or pane delivery"]
    fn whisper_model_is_released_and_can_be_reloaded() {
        fn memory(label: &str) {
            let output = std::process::Command::new("vmmap")
                .args(["-summary", &std::process::id().to_string()])
                .output()
                .unwrap();
            assert!(output.status.success());
            let summary = String::from_utf8_lossy(&output.stdout);
            let footprint = summary
                .lines()
                .find(|line| line.starts_with("Physical footprint:"))
                .unwrap();
            eprintln!("{label}: {footprint}");
        }
        memory("before load");
        for session in 1..=2 {
            let stt = super::Stt::for_session().unwrap();
            memory(&format!("session {session} loaded"));
            drop(stt);
            memory(&format!("session {session} released"));
        }
    }
}
