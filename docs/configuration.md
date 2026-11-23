# Configuration

[← Back to vibe-herdr](../README.md)

Keep personal settings in `~/.config/herdr/plugins/config/ranolp.handsfree/`, outside the repository. Start from [examples/config.toml](../examples/config.toml).

```toml
whisper_model = "small"
language = "ko"
vad = "silero"
microphone = ""
input_gain = 1.0
min_speech_rms = 0.0
phase_accent = false
end_word = ""
```

| Setting | Behavior |
| --- | --- |
| `whisper_model` | Model used by a Whisper build; larger models use more memory. |
| `language` | Recognition language; `auto` lets the model detect it. |
| `vad` | Voice activity detector; the example selects Silero. |
| `microphone` | Device name or case-insensitive substring; empty uses the system input. |
| `input_gain` | Gain applied before the speech-level threshold. |
| `min_speech_rms` | Additional audio-level threshold; `0.0` disables it. |
| `phase_accent` | Optional pane accent changes while dictation is active. |
| `end_word` | Optional spoken override for automatic-mode submission. |

Models download on first use from upstream Hugging Face/GitHub URLs over HTTPS, without pinned checksums. The small multilingual model is roughly 487 MB on disk. The RMS threshold filters quiet audio; it does not cancel background speech or music.

Microphone, language, VAD, and model settings require a daemon restart. Both speech modes unload the model after dictation is off and in-flight work finishes. The next session loads it again; no fixed reload time is guaranteed.

## Space mode

Hold unmodified Space in Ghostty to record, then release to transcribe and submit. This mode does not use TypeSafe Jev. See [setup and permissions](setup.md) for short taps, keyboard interception, and recording readiness.

## Automatic mode

Automatic mode is tuned for Korean conversational turns. It uses voice activity detection to segment speech, then sends buffered transcripts to TypeSafe Jev to judge turn completion. Recognition can be configured for another language, but the completion rules have not been verified for other languages.

Put your own TypeSafe API key in `jev.key` inside the private plugin config directory. Set owner-only permissions:

```sh
chmod 600 ~/.config/herdr/plugins/config/ranolp.handsfree/jev.key
```

With dictation off, select `auto` using `prefix+shift+v`, then enable with `prefix+d`. Saying the configured `end_word` can force submission.

If the classifier fails or reports an incomplete turn, an unsent draft is still submitted after 15 seconds of silence, or 60 seconds when an end word is configured. Turning automatic dictation off discards pending, unsent speech. Switching focus preserves the original destination rather than sending an old draft to the newly focused pane.

## Recognition hints

Put one spelling per line in `glossary.txt`; lines beginning with `#` are comments. The file is reread for each transcript. Hints do not guarantee a particular spelling. See [examples/glossary.txt](../examples/glossary.txt).

## Explicit corrections

Save a correction using the local binary:

```sh
HERDR_PLUGIN_CONFIG_DIR="$(herdr plugin config-dir ranolp.handsfree)" \
  target/release/herdr-handsfree add-correction "wrong text" "correct text"
```

Corrections live in `corrections.tsv` and are reread for each transcript in both modes. Matching is literal and case-sensitive. Edit an existing conflicting correction in the file. This does not train the model.

## Optional status display

[scripts/handsfree-status.py](../scripts/handsfree-status.py) reads the daemon socket and prints the current mode and phase. It does not open the microphone or make network requests. Configure your Herdr status bar separately if you want to use it.

The optional pane accent is red during detected speech and orange during other active phases. Turning dictation off restores the existing theme. No status-bar modification is installed automatically.
