#!/usr/bin/env python3
"""Switch dictation mode while preserving the previous on/off state."""
import json
import os
from pathlib import Path
import socket
import sys
import time

STATE_DIR = Path(os.environ.get("HERDR_PLUGIN_STATE_DIR", Path.home() / ".local/state/herdr-handsfree"))


def request(command, **fields):
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(5)
        connection.connect(str(STATE_DIR / "control.sock"))
        connection.sendall(json.dumps({"cmd": command, **fields}).encode() + b"\n")
        with connection.makefile("rb") as reader:
            line = reader.readline(16384)
    response = json.loads(line)
    if not response.get("ok"):
        raise RuntimeError(response.get("error") or "daemon rejected the request")
    return response["state"]


def switch(call=request, wait=time.sleep):
    initial = call("status")
    was_on = initial["dictation"]
    if was_on:
        allowed = {"listening"}
        if initial.get("dictation_mode") == "space":
            allowed.add("error")
        if initial.get("dictation_phase") not in allowed:
            raise RuntimeError("음성 처리가 끝난 뒤 다시 시도해 주세요")
        wait(0.5)
        current = call("status")
        if not current["dictation"] or current.get("dictation_mode") != initial.get("dictation_mode") or current.get("dictation_phase") not in allowed:
            raise RuntimeError("받아쓰기 상태가 바뀌었습니다. 다시 시도해 주세요")
        call("toggle", target="dictation")
    try:
        state = call("cycle_dictation_mode")
    finally:
        if was_on:
            # Query first: a response timeout does not prove an action failed.
            current = call("status")
            if not current["dictation"]:
                call("toggle", target="dictation")
    return state


if __name__ == "__main__":
    try:
        result = switch()
        print("받아쓰기 모드:", "스페이스" if result["dictation_mode"] == "space" else "자동")
    except (OSError, ValueError, KeyError, RuntimeError) as exc:
        print(f"모드 전환 실패: {exc}", file=sys.stderr)
        sys.exit(1)
