# vibe-herdr

**Speak your next prompt. Keep coding in Herdr.**

Voice input for coding agents inside Herdr. Hold **Space** to speak, then release it to transcribe and submit your prompt to the agent pane you started in.

Built for **macOS Apple Silicon + Ghostty + Herdr**. Speech recognition runs locally with Whisper. An optional automatic mode uses a remote completion classifier. This is a community fork of [RanolP/herdr-handsfree](https://github.com/RanolP/herdr-handsfree), not an official Herdr release.

[![CI](https://github.com/chung-song/vibe-herdr/actions/workflows/ci.yml/badge.svg)](https://github.com/chung-song/vibe-herdr/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

## What you can do

- **Hold Space to talk.** Release to transcribe the recording and submit it. This mode does not use the completion-classifier API.
- **Keep the original destination.** Speech stays bound to the agent pane captured when recording begins, even if focus moves later.
- **Use automatic dictation.** Voice activity detection segments speech; a completion classifier helps decide when a conversational turn is finished.
- **Remember explicit corrections.** Save a misrecognition and its intended text without training the model.
- **Release model memory when off.** Turning dictation off unloads the speech model after in-flight work finishes. Turning it back on reloads the model.
- **See the recording phase.** Optional pane accents distinguish voiced speech from other active phases.

## Quick start

You need macOS Apple Silicon, Ghostty, a working Herdr installation, Rust/Cargo, CMake, and the Xcode command-line tools. The local setup was tested with Herdr 0.9.3; other versions and operating systems are not verified.

```sh
git clone https://github.com/chung-song/vibe-herdr.git
cd vibe-herdr
cargo build --release --no-default-features --features whisper

mkdir -p ~/.config/herdr/plugins/config/ranolp.handsfree
cp examples/config.toml ~/.config/herdr/plugins/config/ranolp.handsfree/config.toml
herdr plugin link "$(pwd)"
```

The plugin keeps the upstream ID `ranolp.handsfree` for compatibility. If the upstream plugin is already linked, unlink that plugin before linking this fork. No prebuilt binary, model, or personal API key is included.

Add the following to your Herdr `config.toml`, normally at `~/.config/herdr/config.toml`. Check for shortcut conflicts first, then run `herdr server reload-config`.

```toml
[[keys.command]]
key = "prefix+d"
type = "plugin_action"
command = "ranolp.handsfree.toggle-dictation"
description = "Toggle voice dictation"

[[keys.command]]
key = "prefix+shift+v"
type = "plugin_action"
command = "ranolp.handsfree.cycle-dictation-mode"
description = "Switch dictation mode"
```

### Talk with Space

1. Allow **Microphone** access when macOS asks. Allow **Accessibility** access for the built `target/release/herdr-handsfree` executable in System Settings → Privacy & Security → Accessibility.
2. With dictation **off**, press `prefix+shift+v` to select `space` mode. The initial mode is `auto`; the selected mode persists across daemon restarts.
3. Press `prefix+d` to enable dictation. Wait for the model to load.
4. Focus your coding-agent pane in Ghostty. Hold the unmodified Space key; wait for recording to start before speaking. Release to transcribe and submit.
5. Press `prefix+d` to turn dictation off when finished. To change modes, turn dictation off first.

A Space press shorter than 250 ms is treated as a regular space when focus remains on the same pane. Modifier shortcuts such as Command-Space are not intercepted. While this mode is on, it intercepts Space in **any Ghostty window**, not only a specific tab. Recording or pending transcription must finish before this mode can be turned off.

If a rebuilt executable loses Accessibility access, remove its old entry and add the current executable again. Bluetooth microphone startup can take time. Model reload time depends on the model, disk cache, and available memory.

## Configuration

Start with [examples/config.toml](examples/config.toml). Put personal settings in the plugin config directory, outside the repository.

```toml
whisper_model = "small"
language = "ko"
vad = "silero"
microphone = ""          # Empty uses the macOS default input
input_gain = 1.0
min_speech_rms = 0.0
phase_accent = false
end_word = ""
```

The documented build selects Whisper with Metal on macOS. Cargo's upstream default backend is Qwen3-ASR, so keep `--no-default-features --features whisper` when following this guide. Models download on first use. `small` is roughly 487 MB on disk; larger models use more memory.

Microphone, language, VAD, and model settings are read by the daemon and require a restart. `glossary.txt` and `corrections.tsv` are reread for each transcript. Glossary entries are recognition hints, not guaranteed spelling overrides. The RMS threshold filters quiet audio; it does not cancel background speech or music.

### Optional automatic mode

Automatic mode is tuned for Korean conversational turns. It sends buffered transcripts to **TypeSafe Jev** to judge whether you have finished speaking. It requires your own TypeSafe API key in `jev.key` inside the private plugin config directory; set owner-only permissions with `chmod 600 jev.key`.

Select `auto` while dictation is off, then enable dictation with `prefix+d`. Saying the configured `end_word` can force submission. If the classifier fails or says the turn is incomplete, an unsent draft is still submitted after 15 seconds of silence, or 60 seconds when an end word is configured. Turning automatic dictation off discards pending, unsent speech.

### Save a correction

```sh
HERDR_PLUGIN_CONFIG_DIR="$(herdr plugin config-dir ranolp.handsfree)"   target/release/herdr-handsfree add-correction "wrong text" "correct text"
```

Corrections are literal and case-sensitive. They apply in both modes, do not train the model, and can be edited in `corrections.tsv`. See [examples/glossary.txt](examples/glossary.txt) for recognition hints and [scripts/handsfree-status.py](scripts/handsfree-status.py) for an optional status display.

## Where your data goes

| Step | Destination |
| --- | --- |
| Speech recognition | Local Whisper model on your Mac |
| Space mode completion | No Jev request |
| Automatic mode completion | Buffered transcript sent to TypeSafe Jev over HTTPS |
| Submitted prompt | Herdr's selected coding agent and its configured provider |
| Diagnostics | Local daemon logs, which can contain recognized text |

This is not an entirely offline assistant: coding agents may use remote providers, and automatic mode sends transcripts to TypeSafe. Keep keys, personal corrections, glossary files, and daemon logs private. Models download from upstream Hugging Face/GitHub URLs over HTTPS without pinned checksums. Review upstream model licenses before redistributing them.

## Development

```sh
cargo test --no-default-features --features whisper -- --skip wav_through_vad_and_stt
python3 -m unittest discover -s tests -p 'test_switch_dictation_mode.py'
git diff --check
```

The excluded English audio fixture has a known exact-transcript mismatch on the development machine. Tests marked ignored require a local model, microphone-related integration setup, or paid classifier access. Passing the default suite does not establish microphone accuracy or live provider delivery. The upstream gaze implementation remains in the source, but the plugin manifest exposes dictation actions only.

If you try this with another microphone, Herdr version, or language, an issue with your setup and reproduction steps is welcome. Remove transcripts and credentials from logs before sharing them.

## Credits and license

Based on [RanolP/herdr-handsfree](https://github.com/RanolP/herdr-handsfree). The upstream MIT license and copyright notice are preserved. See [LICENSE](LICENSE) and [PROVENANCE.md](PROVENANCE.md).
