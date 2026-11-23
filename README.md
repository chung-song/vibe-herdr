# Local Herdr handsfree — Whisper dictation

Local fork of RanolP/herdr-handsfree at upstream commit `61ee20b724ac2b17d63998a41ae9960e4361e7e3`. Herdr 0.9.3 on macOS Apple Silicon. The locally linked manifest has no download/build hook, exposes only `ranolp.handsfree.toggle-dictation`, and runs a locally compiled binary. The binary still contains the upstream gaze implementation, but it has no Herdr action or pane binding.

Build and link:

    cargo build --release --no-default-features --features whisper
    herdr plugin link "$(pwd)"

To use the same behavior on another Mac, install Rust, CMake and Herdr, clone this repository, and run the commands above from the clone. Add the following to `~/.config/herdr/config.toml` and reload Herdr's config:

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

`prefix+d` turns dictation on or off. Press `prefix+shift+v` to switch between `auto` and `space` while idle; it preserves the on/off state. The switch is refused during speech or transcription. The selected mode persists across daemon restarts. In `space` mode, hold the unmodified space bar in Ghostty to record, then release it to transcribe the complete recording and submit it to the agent pane selected at key-down; Jev is not used in this mode. The microphone stream is opened on press and closed on release. Allow macOS Accessibility access for the `herdr-handsfree` executable before selecting this mode. Because this monitors the frontmost Ghostty application rather than one specific tab, space is intercepted in any Ghostty window while the mode is on; short taps are sent as spaces when the focus remains on the same pane. Modifier shortcuts such as Command-Space are not intercepted. A press shorter than 250 ms inserts a space instead of starting transcription; longer presses record speech. Bluetooth microphone startup can take time, so wait for the red indicator before speaking. Background transcription and pending delivery prevent turning this mode off until they finish. This does not change the automatic mode's cancel-on-toggle-off behavior.

To remember a confirmed misrecognition, run `HERDR_PLUGIN_CONFIG_DIR="$(herdr plugin config-dir ranolp.handsfree)" target/release/herdr-handsfree add-correction "wrong text" "correct text"`. The program stores explicit text pairs in `corrections.tsv` in the plugin config directory and rereads them for each transcript in both modes. Matching is literal and case-sensitive; an existing different correction must be edited in the file. Do not add an ambiguous replacement without considering other contexts. This is not automatic learning from conversations and does not train the recognition model.

Create the plugin config directory `~/.config/herdr/plugins/config/ranolp.handsfree/` and put your own TypeSafe API key in `jev.key` there with owner-only permissions (`chmod 600 jev.key`). Never commit the key. For a standalone daemon, set `HERDR_PLUGIN_CONFIG_DIR` to that directory; without it the program uses `~/.config/herdr-handsfree/`. Add a `config.toml` in that directory if you want non-default settings. For example:

```toml
whisper_model = "large-v3-turbo-q5_0"
language = "ko"
vad = "silero"
min_speech_rms = 0.015
input_gain = 3.0
end_word = "이상"
phase_accent = true
```

The microphone defaults to the system input unless `microphone` is set. The example gain and level threshold suit the original setup and may need adjustment. The optional tab-bar status script mentioned below is local configuration and is not included in this repository.

The `--no-default-features` flag matters: default is Qwen3-ASR; `whisper` uses whisper-rs/whisper.cpp with Metal. The `small` multilingual ggml model (~487 MB) and silero VAD model download on first actual use. These model files come from upstream Hugging Face and GitHub URLs in `src/models.rs` (no pinned digest); builds and models are not cryptographically verified beyond HTTPS. The release binary is built locally; do not use upstream's prebuilt release download hook.

On the original Mac, `config.toml` selects `large-v3`, Korean, and `DJI Mic Mini-D14997`. The microphone and gain settings are machine-specific. Earlier tests found JBL Charge 4 audio dropouts when the DJI Bluetooth microphone was open; the underlying mechanism was not measured. `min_speech_rms` adds a minimum level alongside Silero's speech probability. This is a level filter, not acoustic echo cancellation, and cannot reliably separate the user's voice from loud media. Edit `glossary.txt` in the config directory, one name per line (comments start with `#`). The glossary is re-read for each utterance and is a probabilistic recognition hint, not a guaranteed spelling override. Changing model/mic/VAD/language requires a daemon restart; glossary edits do not. The speech model is owned by the active dictation session, rather than cached for the daemon lifetime. Turning dictation off releases it once any in-flight load or transcription finishes; the next toggle-on reloads it. Both automatic and space dictation follow this policy.

Shortcut: `Ctrl+B`, then `d` (`prefix+d`) toggles listening across Herdr agent panes. The Herdr tab-bar status script shows `🎙 ON`, `🎙 OFF`, or `🎙 ?`; Herdr 0.9.3 command widgets do not expose a click action, so `prefix+d` remains the toggle. The pane accent is red only on voiced VAD frames and orange for every other active phase, including listening, settling, transcription, checking, waiting, and error; dictation off restores the theme. Red changes to orange on the first unvoiced frame. Speech remains bound to the agent pane captured at onset across focus changes and transcription, then is delivered to that original pane rather than the newly focused one. The VAD end-of-speech debounce is approximately 1.5 seconds; switching panes during the silent settling interval flushes the old utterance early. You can therefore switch as soon as red turns orange; actual text insertion still waits for transcription and turn processing. The focus monitor polls every 50 ms, so extremely fast focus changes may not be observed. Toggle off to discard pending drafts. `prefix+shift+d` remains Herdr's close-workspace shortcut. An OS microphone permission prompt may require manual approval. The `DJI Mic Mini-D14997` input remains selected; its JBL Bluetooth output dropouts are not solved by this change.

The completion classifier sends buffered transcripts to TypeSafe Jev's HTTPS API. The owner-only `jev.key` file supplies the credential. On an API failure or an incomplete verdict the draft waits for more speech; after 15 seconds of silence, or 60 seconds when an end word is configured, the draft is delivered to its original pane. Saying the configured end word forces delivery sooner. The daemon log can contain recognized transcripts and should be treated as private.

Validation:

    cargo test --no-default-features --features whisper -- --skip wav_through_vad_and_stt
    HERDR_PLUGIN_CONFIG_DIR=/Users/eroh3000/.config/herdr/plugins/config/ranolp.handsfree cargo test --no-default-features --features whisper --bin herdr-handsfree jev_classifies_an_unfinished_and_a_complete_turn -- --ignored
    cargo test --no-default-features --features whisper --bin herdr-handsfree synthesized_speech_crossing_panes_reaches_only_the_original_agent -- --ignored
    herdr config check
    herdr plugin action list --plugin ranolp.handsfree

On this Mac, `wav_through_vad_and_stt` fails its exact `hello world` transcript assertion and instead decodes `필로월드 디스위즈어 디테이션 테스트`; the Korean streaming/batch test passes. Jev API tests require user-provided credit and are ignored. The isolated synthesized-audio cross-pane test is also ignored and was not rerun for this patch; it uses a fake Herdr command and CPU Whisper and now asserts delivery only to the original pane, never to the newly focused pane.

A synthetic Korean `say -v Yuna` WAV was successfully transcribed offline by the release binary with both tiny and small models, including after the added RMS gate. With the old laptop mic and no extra RMS gate, a live YouTube-background probe showed unintentional speech detection; switching to DJI Mic Mini alone still transcribed background speech in a 6-second probe. After adding `min_speech_rms = 0.01`, a bounded 6-second probe using DJI selected that device, loaded Whisper, reached `listening`, and produced no transcript. After the JBL dropout report, two bounded 8-second ffmpeg input-only trials reproduced dropouts with DJI and not with the built-in mic. A further plugin probe with the built-in mic reached `listening` and `speech`, produced no transcript in that short sample, and did not cause audible JBL dropout (user-confirmed). Delivery was deliberately disabled through `HERDR_BIN_PATH=/usr/bin/false` during plugin probes. These trials do not establish near-mic voice accuracy or reliable loud-media rejection; test with the user present. The plugin was left off and the normal daemon restored. Offline logs: `/Users/eroh3000/.hermes/cache/scratch/handsfree-small-smoke.log` (scratch may be pruned).

Rollback without touching unrelated config: remove the `prefix+d` `[[keys.command]]` block and only the `/Users/eroh3000/.config/herdr/handsfree-status.py` command entry in `tab_bar_right` from `/Users/eroh3000/.config/herdr/config.toml`, then `herdr server reload-config`; run the binary's `stop` subcommand with `HERDR_PLUGIN_STATE_DIR=/Users/eroh3000/.local/state/herdr/plugins/ranolp.handsfree` and `herdr plugin unlink ranolp.handsfree`. Then remove the unused status script if desired. Original config snapshot (101 lines) is `/Users/eroh3000/.config/herdr/config.toml.pre-handsfree-20260925.bak`, but do not blindly restore it after unrelated edits. Remove the model/config directories only if you explicitly want to discard cached models and glossary. Do not stop Herdr itself.
