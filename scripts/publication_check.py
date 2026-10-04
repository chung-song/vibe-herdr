#!/usr/bin/env python3
"""Pre-publication source scan; fail on common personal paths and secrets.

This is a heuristic, not a substitute for gitleaks or manual review.
Only non-ignored Git candidate files are scanned, not local config or models.
"""
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
PATTERNS = {
    "macOS personal home path": re.compile(r"/Users/[A-Za-z0-9_.-]+/"),
    "Linux personal home path": re.compile(r"/home/[A-Za-z0-9_.-]+/"),
    "email address": re.compile(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b"),
    "private key header": re.compile(r"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----"),
    "probable credential assignment": re.compile(
        r"(?i)\b(?:api[_-]?key|token|password|secret)\s*[=:]\s*['\"]?(?!\s*(?:<|\$|\{|None\b|os\.|std::|env\())"
        r"[A-Za-z0-9_+./=-]{20,}"
    ),
    "GitHub token": re.compile(r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{20,}\b"),
}


def candidates():
    output = subprocess.check_output(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
        cwd=ROOT,
    )
    return sorted(set(Path(p.decode()) for p in output.split(b"\0") if p))


def main():
    findings = []
    files = candidates()
    for relative in files:
        path = ROOT / relative
        if not path.is_file():
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (UnicodeError, OSError):
            findings.append((str(relative), 0, "non-text/unreadable candidate; manual review required"))
            continue
        for lineno, line in enumerate(text.splitlines(), 1):
            for label, pattern in PATTERNS.items():
                if pattern.search(line):
                    findings.append((str(relative), lineno, label))
    for path, lineno, label in findings:
        print(f"{path}:{lineno}: {label}")
    print(f"scanned {len(files)} Git candidate files; findings={len(findings)}")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
