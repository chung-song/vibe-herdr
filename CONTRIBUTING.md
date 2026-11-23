# Contributing to vibe-herdr

Thanks for trying voice input in Herdr. Useful contributions include microphone reports, clearer setup instructions, regression tests, and focused fixes.

## Report an issue

Include your macOS version, chip, Herdr version, microphone, speech mode, model, and steps to reproduce. Describe the expected and actual behavior. Remove keys, private transcripts, and unrelated log lines before sharing diagnostics.

If a recording went to the wrong pane, include when focus moved relative to key-down, speech, key-up, and transcription. Never include another person's private conversation.

## Work locally

Follow [setup](docs/setup.md) and use an isolated test configuration. Do not run integration tests against a live agent pane unless you intend to submit a prompt.

```sh
cargo test --no-default-features --features whisper -- --skip wav_through_vad_and_stt
python3 -m unittest discover -s tests -p 'test_switch_dictation_mode.py'
git diff --check
python3 scripts/publication_check.py
```

The excluded English fixture has a known exact-transcript mismatch on the development machine. Tests marked ignored need a local model, additional integration setup, or paid classifier access. Passing the default suite does not establish microphone accuracy or live provider delivery. CI covers Whisper and Qwen backends.

Keep pull requests focused. Explain the behavior change and the validation you ran. Preserve the upstream MIT license and attribution in [PROVENANCE.md](PROVENANCE.md).

## README artwork

The light and dark SVGs in `docs/assets/` are original, editable illustrations. They describe the push-to-talk flow and are not application screenshots. Keep visible labels and descriptions accurate if you change them.
