#!/usr/bin/env python3
"""Read-only Herdr handsfree status; no microphone or network access."""
import json
import os
import socket
from pathlib import Path

SOCKET = Path(os.environ.get(
    "HANDSFREE_STATUS_SOCKET",
    str(Path(os.environ.get("HERDR_PLUGIN_STATE_DIR", Path.home() / ".local/state/herdr/plugins/ranolp.handsfree")) / "control.sock"),
))
LABELS = {
    "off": "voice off",
    "preparing": "voice preparing",
    "listening": "voice listening",
    "speech": "voice detected",
    "transcribing": "voice transcribing",
    "checking": "voice checking",
    "waiting": "voice waiting",
    "error": "voice error",
}

def status_label(sock_path=SOCKET):
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as conn:
            conn.settimeout(0.3)
            conn.connect(str(sock_path))
            conn.sendall(b'{"cmd":"status"}\n')
            data = b""
            while not data.endswith(b"\n") and len(data) < 4096:
                chunk = conn.recv(4096)
                if not chunk:
                    break
                data += chunk
        state = json.loads(data)["state"]
        phase = state.get("dictation_phase", "preparing" if state.get("dictation") else "off")
        mode = "space" if state.get("dictation_mode") == "space" else "auto"
        return f"{mode} {LABELS.get(phase, 'voice unknown')}"
    except (OSError, ValueError, KeyError, TypeError):
        return "voice standby"

if __name__ == "__main__":
    print(status_label())
