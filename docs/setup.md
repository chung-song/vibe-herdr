# Setup

[← Back to vibe-herdr](../README.md)

## Requirements

- macOS on Apple Silicon, Ghostty, and a working Herdr installation.
- Rust/Cargo, CMake, Python 3, and the Xcode command-line tools.
- Network access for dependencies and the first model download.

The local setup was tested with Herdr 0.9.3. Other versions and operating systems are not verified. This fork keeps the upstream plugin ID `ranolp.handsfree`; if the upstream plugin is linked, unlink it before linking this fork.

## Build and link

```sh
git clone https://github.com/chung-song/vibe-herdr.git
cd vibe-herdr
cargo build --release --no-default-features --features whisper

mkdir -p ~/.config/herdr/plugins/config/ranolp.handsfree
cp examples/config.toml ~/.config/herdr/plugins/config/ranolp.handsfree/config.toml
herdr plugin link "$(pwd)"
```

The explicit build flags select Whisper with Metal. Cargo's upstream default backend is Qwen3-ASR. This repository does not include model weights or a prebuilt binary.

## Add shortcuts

Add [examples/herdr-keybinding.toml](../examples/herdr-keybinding.toml) to your Herdr config, normally `~/.config/herdr/config.toml`. Check for shortcut conflicts first.

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

Reload Herdr's configuration:

```sh
herdr server reload-config
```

`prefix` means your configured Herdr prefix. The default setup uses Ctrl+B.

## Allow macOS permissions

Allow Microphone access when macOS asks. For space-held dictation, open System Settings → Privacy & Security → Accessibility and add this repository's built `target/release/herdr-handsfree` executable.

If a rebuilt executable loses access, remove its old entry and add the current executable again. A switch that looks enabled does not always mean a replacement executable has access.

## First recording

1. With dictation **off**, use `prefix+shift+v` to select `space`. The initial mode is `auto`; the selected mode persists across daemon restarts.
2. Use `prefix+d` to turn dictation on. Wait for the model to load.
3. Focus the coding-agent pane in Ghostty. Hold unmodified Space and wait for recording to start before speaking.
4. Release Space to transcribe and submit to the pane captured at key-down.
5. Turn dictation off with `prefix+d` when finished. Turn it off before changing modes.

A Space press shorter than 250 ms is treated as a regular space when focus remains on the same pane. Modifier shortcuts such as Command-Space are not intercepted. While the mode is on, Space is intercepted in **any Ghostty window**, not just one tab. Pending transcription or delivery must finish before space mode can be turned off.

## Troubleshooting

| Symptom | Check |
| --- | --- |
| Space mode will not enable | Add the current executable to Accessibility. |
| First words are missing | Bluetooth input startup may be slow; wait for recording readiness. |
| Wrong microphone | Set `microphone` in the private config. An empty value uses the system default. |
| Model startup is slow | Larger models and low free memory can increase reload time. |
| Mode switch is refused | Turn dictation off first; let space-mode work finish. |
| Background voices appear | Tune the input and RMS threshold. This does not cancel background speech. |

The model is released after dictation turns off and in-flight work finishes. Turning it back on reloads the model. Loading and recognition are synchronous and may finish after an off request.

For microphone, model, VAD, and language changes, restart the handsfree daemon. Glossary and correction changes are reread per transcript. For a standalone daemon, set `HERDR_PLUGIN_CONFIG_DIR` to your plugin config directory; without it, configuration defaults to `~/.config/herdr-handsfree/`.
