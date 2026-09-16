# claude.ai export automation — setup & gotchas

Fully-automatic corpus refresh: request a claude.ai data export weekly, catch the ready
email, download the zip, index it, and fill the consolidation review queue. Removes the
"remember to request + download the export" toil. **Proven end-to-end 2026-07-11.**

## Architecture

```
cortex-export-request.timer  (weekly) ─► run-export.sh request ─► export-bot.js request  (headed Playwright)
                                                                        │
                                        claude.ai processes ASYNC ──► emails the ready link
                                                                        │
cortex-export-retrieve.timer (every 4h) ─► run-export.sh retrieve ─► retrieve.py  (Gmail IMAP → URL)
                                                                  ─► export-bot.js download <url> ─► ~/Downloads/data-*.zip
                                                                        │
cortex-corpus-refresh.path (watch ~/Downloads) ─► refresh-corpus.sh ─► ecs index + semantic-index ─► chat-consolidate.sh
                                                                        │
                                                           review queue ─► cortex_review.py  (you drain)
```

## One-time setup

### 1. Playwright browser
```sh
npx playwright install chromium     # ~150MB (already installed on this box)
```

### 2. claude.ai session cookie (auth — no GUI needed)
Headed GUI login does **not** render on WSL (WSLg is flaky), so import the cookie instead:
1. In a browser logged into claude.ai: DevTools (F12) → **Application** → **Cookies** → `https://claude.ai` → copy the **`sessionKey`** value.
2. Save it without touching shell history, then fold it into the auth state:
   ```sh
   cd scripts/export-automation
   read -rs SK && printf '%s' "$SK" > .session-cookie && unset SK
   node export-bot.js import-cookie      # builds storageState.json, wipes .session-cookie
   node export-bot.js discover           # verify: loggedIn=true + "Export data" button
   ```

### 3. Gmail app-password (headless retrieval)
The Gmail claude.ai **connector is NOT available in headless `claude -p`** (see gotcha 4), so
retrieval uses IMAP:
1. Google Account → Security → 2-Step Verification → **App passwords** → generate one.
2. Ensure **IMAP is enabled** in Gmail (Settings → Forwarding and POP/IMAP).
3. Add to `~/.cortex-consolidate.env`:
   ```sh
   export CORTEX_GMAIL_APP_PASSWORD='xxxx xxxx xxxx xxxx'
   export CORTEX_GMAIL_ADDR='you@gmail.com'               # your Gmail address
   export CORTEX_CONSOLIDATE_NTFY='<ntfy-topic>'          # optional: push alerts
   ```

### 4. Enable the timers
```sh
cp scripts/systemd/cortex-export-*.{service,timer} scripts/systemd/cortex-corpus-refresh.* ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now cortex-export-request.timer cortex-export-retrieve.timer cortex-corpus-refresh.path
```

## GOTCHAS (all verified 2026-07-11 — don't relearn these the hard way)

1. **Cloudflare blocks HEADLESS** ("Just a moment…" challenge). Launch **headed**
   (`headless:false`) — on WSL it renders to the WSLg display *invisibly*, which passes the
   challenge; request/download need no visible window (auth is the cookie). Launch args:
   `--no-sandbox`, `--disable-blink-features=AutomationControlled`.
2. **claude.ai is an SPA that never reaches `networkidle`** → use `waitUntil:'domcontentloaded'`
   + a fixed `waitForTimeout(~3500)`. `networkidle` always times out.
3. **WSLg headed GUI windows may not appear** → don't rely on a visible login; import the
   `sessionKey` cookie (step 2).
4. **Gmail connector is NOT available in headless `claude -p`** — returns `GMAIL_UNAVAILABLE`
   even with `--dangerously-skip-permissions`. claude.ai connectors don't survive headless/cron
   (unlike registered/plugin MCPs). Use IMAP (`retrieve.py`).
5. **`sessionKey` cookie EXPIRES** (weeks). Then request/download fail with a login redirect →
   `run-export.sh` alerts via ntfy → **re-import the cookie (step 2)**. This is the one ongoing
   maintenance chore.
6. **Export is ASYNC** — the email arrives minutes–hours after the request. That's why request
   (weekly) and retrieve (4-hourly) run on separate schedules.
7. **No in-app download** — claude.ai delivers the link *only* by email; there is no in-app
   export history to scrape.
8. **node under systemd** — node is fnm-managed with a *session-specific* path that won't exist
   in a service. The units put the **stable** `~/.local/share/fnm/aliases/default/bin` on PATH.
9. **The export UUID is stable per account; only the per-export download token changes.** Always
   match the full URL from the latest email — don't reuse a cached one.

## Manual operation
```sh
cd scripts/export-automation
node export-bot.js request               # request an export now
bash run-export.sh retrieve              # check email + download if ready (needs app-password)
node export-bot.js import-cookie         # re-auth after cookie expiry (step 2 first)
```
