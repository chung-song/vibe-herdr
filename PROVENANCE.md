# Source and attribution for vibe-herdr

- Intended public repository name: `vibe-herdr`. GitHub name availability has not been checked; no remote has been created or pushed.

- Upstream: `RanolP/herdr-handsfree`, upstream Git commit `61ee20b724ac2b17d63998a41ae9960e4361e7e3` (`origin/main` at discovery).
- Local modification branch at discovery: `local/voice-turn-gate`, commit `1e65e42` (`Deliver an expired unsent draft instead of discarding it`). The source worktree was clean. The manifest ID remained `ranolp.handsfree` and was locally linked.
- Scope relative to upstream: Rust voice turn buffering, Whisper option/glossary, mic/VAD controls, remote TypeSafe Jev completion, focus-aware delivery, status/accent behavior, manifest/README. This preparation further sanitizes routine logs and documentation, and omits automatic release publishing.
- The upstream MIT LICENSE is retained verbatim with copyright `2026 RanolP`; `Cargo.toml` also identifies MIT. This derivative must keep that notice. Proposed license for contributions: continue MIT for compatibility, but the new-contribution owner/licensing decision is pending explicit user approval. No endorsement by upstream or Herdr is implied.
- Discovery: the active Herdr `plugins.json` entry named the local manifest and root, and the source repository's Git metadata confirmed upstream owner/branch. Full machine-specific discovery paths and verification log are in an ignored local preparation note, not in publication content. No `.git` history from the source, local config, keys, glossary, model files or binaries are copied into this candidate.
