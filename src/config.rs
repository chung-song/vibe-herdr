//! User config: `config.toml` in HERDR_PLUGIN_CONFIG_DIR. Every field is
//! optional; env vars HANDSFREE_MODEL / HANDSFREE_VAD override the file.

use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Qwen3-ASR model size: 0.6B | 1.7B
    pub asr_model: String,
    /// Whisper ggml model size, only used with `--features whisper`:
    /// tiny | base | small | medium | large-v3 | large-v3-turbo | large-v3-turbo-q5_0
    pub whisper_model: String,
    /// Language hint: "auto", a language name like "korean", or an ISO 639-1
    /// code like "ko"
    pub language: String,
    /// Voice activity detector: "silero" | "energy"
    pub vad: String,
    /// Minimum frame RMS required in addition to Silero speech probability.
    /// Helps reject distant speaker bleed. 0 disables the extra floor.
    pub min_speech_rms: f32,
    /// Input device to capture from; case-insensitive substring of the device
    /// name. Empty = system default input.
    pub microphone: String,
    /// Software gain applied to captured audio before VAD and STT, for quiet
    /// microphones. Samples are clipped to [-1, 1].
    pub input_gain: f32,
    /// Tint Herdr's accent color (focused pane border) by dictation phase:
    /// red on voiced frames, orange while settling/transcribing/judging or
    /// waiting for more speech. Edits one marked line in Herdr's config.
    pub phase_accent: bool,
    /// Spoken word that forces submission when the remote completion judge
    /// misses a finished turn, e.g. "이상". Empty disables the override.
    pub end_word: String,
    /// Bundle ID of the terminal where space push-to-talk is active.
    pub push_to_talk_app: String,
    /// One-Euro filter: lower = smoother but laggier cursor
    pub smoothing_min_cutoff: f64,
    /// One-Euro filter: higher = snappier during fast gaze moves
    pub smoothing_beta: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            asr_model: "1.7B".to_string(),
            whisper_model: "small".to_string(),
            language: "auto".to_string(),
            vad: "silero".to_string(),
            min_speech_rms: 0.0,
            microphone: String::new(),
            input_gain: 1.0,
            phase_accent: false,
            end_word: String::new(),
            push_to_talk_app: "com.mitchellh.ghostty".to_string(),
            smoothing_min_cutoff: 1.0,
            smoothing_beta: 0.01,
        }
    }
}

pub fn get() -> &'static Config {
    static CONFIG: OnceLock<Config> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let path = config_dir().join("config.toml");
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                eprintln!("invalid {} ({e}); using defaults", path.display());
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    })
}

pub(crate) fn config_dir() -> std::path::PathBuf {
    std::env::var_os("HERDR_PLUGIN_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").expect("HOME not set"))
                .join(".config/herdr-handsfree")
        })
}

/// Explicit, user-provided transcription corrections. Read on each result so
/// a new correction takes effect without restarting or retaining recordings.
pub(crate) fn correct_transcript(text: &str) -> String {
    let path = config_dir().join("corrections.tsv");
    match std::fs::read_to_string(&path) {
        Ok(rules) if rules.len() <= 16_384 => apply_corrections(text, &rules),
        Ok(_) => {
            eprintln!("corrections.tsv exceeds 16384 bytes; corrections not applied");
            text.to_string()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => text.to_string(),
        Err(e) => {
            eprintln!("cannot read corrections.tsv: {e}");
            text.to_string()
        }
    }
}

fn apply_corrections(text: &str, rules: &str) -> String {
    let mut pairs: Vec<(&str, &str)> = rules
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| line.split_once('\t'))
        .filter(|(heard, intended)| !heard.is_empty() && !intended.is_empty())
        .collect();
    pairs.sort_by_key(|(heard, _)| std::cmp::Reverse(heard.len()));
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        if let Some((heard, intended)) = pairs.iter().find(|(heard, _)| rest.starts_with(heard)) {
            result.push_str(intended);
            rest = &rest[heard.len()..];
        } else {
            let ch = rest.chars().next().expect("nonempty");
            result.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    result
}

/// Append one explicit correction without saving the spoken audio.
pub(crate) fn add_correction(heard: &str, intended: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if heard.trim().is_empty()
        || intended.trim().is_empty()
        || heard.contains(['\t', '\r', '\n'])
        || intended.contains(['\t', '\r', '\n'])
        || heard.trim_start().starts_with('#')
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "use nonempty single-line text without tabs",
        ));
    }
    let dir = config_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("corrections.tsv");
    if let Ok(meta) = std::fs::metadata(&path)
        && meta.permissions().mode() & 0o077 != 0
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "corrections.tsv must be readable only by its owner",
        ));
    }
    let prior = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    if prior.len() + heard.len() + intended.len() + 3 > 16_384 {
        return Err(std::io::Error::other(
            "corrections.tsv would exceed 16384 bytes",
        ));
    }
    for (old, value) in prior.lines().filter_map(|line| line.split_once('\t')) {
        if old == heard {
            return if value == intended {
                Ok(())
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "a different correction already exists; edit corrections.tsv",
                ))
            };
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    if !prior.is_empty() && !prior.ends_with('\n') {
        file.write_all(b"\n")?;
    }
    writeln!(file, "{heard}\t{intended}")
}

#[cfg(test)]
mod correction_tests {
    #[test]
    fn explicit_rules_choose_longest_match_without_cascading() {
        let rules = "# heard<TAB>intended\n노션TV\t노션 DB\n노션\tNotion\n노션 DB\twrong\n";
        assert_eq!(
            super::apply_corrections("노션TV에 기록", rules),
            "노션 DB에 기록"
        );
        assert_eq!(super::apply_corrections("다른 말", rules), "다른 말");
        assert_eq!(
            super::apply_corrections("노션TV 노션", rules),
            "노션 DB Notion"
        );
    }
}

/// Re-read at each utterance so glossary.txt can be edited without restarting.
/// Whisper's initial prompt is a recognition hint, not a spelling guarantee.
#[cfg(feature = "whisper")]
pub fn whisper_glossary() -> std::io::Result<String> {
    let path = config_dir().join("glossary.txt");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(e) => return Err(e),
    };
    if bytes.len() > 4096 {
        return Err(std::io::Error::other("glossary.txt exceeds 4096 bytes"));
    }
    let text = String::from_utf8(bytes).map_err(std::io::Error::other)?;
    if text.contains('\0') {
        return Err(std::io::Error::other("glossary.txt contains a NUL byte"));
    }
    Ok(glossary_prompt(&text))
}

#[cfg(feature = "whisper")]
fn glossary_prompt(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(all(test, feature = "whisper"))]
mod tests {
    #[test]
    fn glossary_skips_comments_and_preserves_spelling() {
        assert_eq!(
            super::glossary_prompt("# names\n피어나\n\n JLPTinder \n"),
            "피어나, JLPTinder"
        );
    }
}
