#!/usr/bin/env python3
"""retrieve.py — headless retrieval of the claude.ai export download URL via Gmail IMAP.

WHY IMAP (and not a headless `claude -p` with the Gmail connector)?
  Empirically verified 2026-07-11: the Gmail claude.ai *connector* is NOT available in
  headless `claude -p` — it returns GMAIL_UNAVAILABLE even with --dangerously-skip-permissions.
  claude.ai connectors don't survive headless/cron (unlike registered/plugin MCPs). IMAP is
  the robust, dependency-free, cron-safe path. (stdlib imaplib + email only.)

Config (export in ~/.cortex-consolidate.env):
  CORTEX_GMAIL_ADDR          Gmail address (required)
  CORTEX_GMAIL_APP_PASSWORD  Gmail APP PASSWORD (Google Account > Security > 2-Step > App passwords).
                             NOT your normal password. IMAP must be enabled in Gmail settings.

Prints the newest UNREAD export download URL to stdout (and marks that email read so it is
not re-downloaded). Exit 3 if no new export email; 2 on config error.
"""
import email
import imaplib
import os
import re
import sys

ADDR = os.environ.get("CORTEX_GMAIL_ADDR", "")
PW = os.environ.get("CORTEX_GMAIL_APP_PASSWORD", "")
URL_RE = re.compile(r"https://claude\.ai/export/[0-9a-f-]+/download/[0-9a-f]+")


def main():
    if not PW or not ADDR:
        print("CORTEX_GMAIL_ADDR / CORTEX_GMAIL_APP_PASSWORD not set (see this file's header)", file=sys.stderr)
        return 2
    try:
        M = imaplib.IMAP4_SSL("imap.gmail.com")
        M.login(ADDR, PW)
    except imaplib.IMAP4.error as e:
        print(f"IMAP login failed: {e} (app-password wrong, or IMAP not enabled in Gmail)", file=sys.stderr)
        return 2
    try:
        M.select("INBOX")
        _, data = M.search(None, '(UNSEEN FROM "mail.anthropic.com" SUBJECT "data is ready")')
        ids = data[0].split()
        if not ids:
            print("no new export email", file=sys.stderr)
            return 3
        latest = ids[-1]  # newest
        _, msgdata = M.fetch(latest, "(RFC822)")
        if not msgdata or not isinstance(msgdata[0], tuple):
            print("could not fetch export email body", file=sys.stderr)
            return 4
        msg = email.message_from_bytes(msgdata[0][1])
        html = ""
        for part in msg.walk():
            if part.get_content_type() == "text/html":
                payload = part.get_payload(decode=True)
                if isinstance(payload, (bytes, bytearray)):
                    html = payload.decode("utf-8", "replace")
                break
        m = URL_RE.search(html)
        if not m:
            print("export email found but no download URL in it", file=sys.stderr)
            return 4
        M.store(latest, "+FLAGS", "\\Seen")  # mark read so we don't re-download
        print(m.group(0))
        return 0
    finally:
        try:
            M.logout()
        except Exception:
            pass


if __name__ == "__main__":
    sys.exit(main())
