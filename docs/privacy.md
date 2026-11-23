# Data flow and limitations

[← Back to vibe-herdr](../README.md)

## Where data goes

| Step | Destination |
| --- | --- |
| Whisper speech recognition | Local model on your Mac. |
| Space mode completion | No Jev request. |
| Automatic mode completion | Buffered transcript sent to TypeSafe Jev over HTTPS. |
| Submitted prompt | Herdr's selected coding agent and its configured provider. |
| Diagnostics | Local daemon logs, which can contain recognized text. |

Local transcription does not make the whole assistant offline. Your coding agent may use a remote provider. Automatic mode sends text to TypeSafe; the local Whisper recognition step and Jev completion request do not intentionally upload microphone audio.

## Keep private

Store API keys, personal settings, corrections, and glossary files outside the repository. Do not attach private transcripts or raw daemon logs to public issues. No API key, personal runtime config, recording, model weight, or compiled binary is committed here.

## Know before using

- Space mode intercepts the unmodified Space key across Ghostty windows while enabled. Short taps are treated as spaces when the original pane remains focused. Recording or transcription must finish before turning this mode off.
- Automatic mode can submit an incomplete draft after its silence timeout, including when the completion API fails. Turning automatic mode off cancels pending, unsent speech.
- A recording is bound to the agent pane captured at onset. This protects the destination during focus changes; it does not prove that a transcript is correct.
- No acoustic echo cancellation is implemented. Loud media and nearby speech can be recognized. Bluetooth input startup can delay recording readiness.
- Model downloads use upstream HTTPS URLs without pinned checksums. Model licenses and dependency licenses remain their own.
- macOS Apple Silicon with Herdr 0.9.3 is the tested setup. Other operating systems, Herdr versions, microphones, and languages are not broadly verified.
- The retained upstream gaze code is not exposed as a Herdr action in the plugin manifest.

For setup and microphone selection, see [Setup](setup.md) and [Configuration](configuration.md).
