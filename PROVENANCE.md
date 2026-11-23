# Source and attribution

`vibe-herdr` is a community fork of [RanolP/herdr-handsfree](https://github.com/RanolP/herdr-handsfree). It is not an official Herdr release.

- The source fork was based on upstream commit `61ee20b724ac2b17d63998a41ae9960e4361e7e3`.
- The current local source snapshot was imported in [b76a4e0](https://github.com/chung-song/vibe-herdr/commit/b76a4e05de0791bc77a2e37b8cb01a25f0455750), preserving this repository's existing history.
- Changes include Whisper dictation, conversational turn buffering, original-pane delivery, microphone/VAD controls, space-held dictation, explicit corrections, pane accents, and releasing the speech model when dictation is off.
- The upstream plugin ID `ranolp.handsfree` is retained for compatibility. The upstream gaze code remains, but the manifest exposes dictation actions only.
- The original MIT license and `Copyright (c) 2026 RanolP` notice are retained verbatim. No endorsement by upstream or Herdr is implied.
- No personal runtime config, API keys, recordings, model weights, or compiled binaries are committed. Diagnostic logs can contain recognized text and should not be published.
- Model weights and dependencies come from their own upstream sources and licenses. Automatic dictation sends buffered transcripts to TypeSafe Jev; submitted prompts use the selected coding agent's provider.
