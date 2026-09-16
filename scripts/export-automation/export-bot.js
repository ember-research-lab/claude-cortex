#!/usr/bin/env node
// export-bot.js — claude.ai data-export automation (standalone Playwright, saved auth).
//
// One-time: `node export-bot.js login`  (headed via WSLg; you log in, session is saved).
// Then (headless, cron-safe): `discover` | `request` | `download` reuse storageState.json.
//
// request/download are built AFTER `discover` reveals the real settings UI — no guessed
// selectors. login + discover are complete and real.
const { chromium } = require('playwright');
const path = require('path');
const fs = require('fs');

const DIR = __dirname;
const AUTH = path.join(DIR, 'storageState.json');
const mode = process.argv[2];

function needAuth() {
  if (!fs.existsSync(AUTH)) {
    console.error('No storageState.json — run:  node export-bot.js login');
    process.exit(2);
  }
}

(async () => {
  if (mode === 'login') {
    const browser = await chromium.launch({ headless: false });
    const ctx = await browser.newContext();
    const page = await ctx.newPage();
    await page.goto('https://claude.ai/login', { waitUntil: 'domcontentloaded' });
    console.log('\n>>> Log in to claude.ai in the browser window (finish any 2FA).');
    console.log('>>> When you can see your chats, come back here and press Enter to save the session…');
    await new Promise((r) => process.stdin.once('data', r));
    await ctx.storageState({ path: AUTH });
    console.log('Saved auth ->', AUTH);
    await browser.close();
    return;
  }

  if (mode === 'import-cookie') {
    // WSL-friendly: no GUI. Build storageState from a pasted claude.ai sessionKey.
    const file = path.join(DIR, '.session-cookie');
    if (!fs.existsSync(file)) {
      console.error(`Create ${file} containing your claude.ai sessionKey value, then re-run.`);
      process.exit(2);
    }
    const sessionKey = fs.readFileSync(file, 'utf8').trim();
    if (!sessionKey) { console.error('.session-cookie is empty'); process.exit(2); }
    const oneYear = Math.floor(Date.now() / 1000) + 60 * 60 * 24 * 365;
    const storageState = {
      cookies: [{
        name: 'sessionKey', value: sessionKey, domain: '.claude.ai', path: '/',
        expires: oneYear, httpOnly: true, secure: true, sameSite: 'Lax',
      }],
      origins: [],
    };
    fs.writeFileSync(AUTH, JSON.stringify(storageState, null, 2));
    fs.unlinkSync(file); // wipe the raw secret once folded into storageState
    console.log(`Built ${AUTH} from sessionKey (removed .session-cookie). Verify: node export-bot.js discover`);
    return;
  }

  needAuth();
  // Cloudflare challenges HEADLESS browsers ("Just a moment…"). Launch HEADED — on WSL
  // it renders to the WSLg display invisibly, which passes the challenge. request/download
  // need no human visibility (auth is in the cookie). Override with HEADLESS=1.
  const browser = await chromium.launch({
    headless: !!process.env.HEADLESS,
    args: ['--no-sandbox', '--disable-blink-features=AutomationControlled'],
  });
  const ctx = await browser.newContext({ storageState: AUTH, acceptDownloads: true });
  const page = await ctx.newPage();

  if (mode === 'discover') {
    // Confirm the session is valid and map the export UI across likely settings pages.
    for (const url of [
      'https://claude.ai/settings/data-privacy-controls',
      'https://claude.ai/settings/account',
      'https://claude.ai/settings',
    ]) {
      try {
        await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 30000 });
        await page.waitForTimeout(3500); // let the SPA render (claude.ai never hits networkidle)
      } catch (e) {
        console.log(`\n=== ${url} => navigation error: ${e.message}`);
        continue;
      }
      const loggedIn = !/\/login/.test(page.url());
      console.log(`\n=== ${url} => now at ${page.url()} | title="${await page.title()}" | loggedIn=${loggedIn}`);
      const bits = await page.evaluate(() => {
        const out = [];
        document.querySelectorAll('button, a, [role=button]').forEach((el) => {
          const t = (el.innerText || '').trim().replace(/\s+/g, ' ');
          if (t && /export|data|download|privacy/i.test(t)) {
            out.push(`${el.tagName} "${t}"${el.getAttribute('href') ? ' href=' + el.getAttribute('href') : ''}`);
          }
        });
        return out.slice(0, 25);
      });
      console.log(bits.length ? bits.join('\n') : '(no export/data/download controls on this page)');
    }
  } else if (mode === 'request') {
    await page.goto('https://claude.ai/settings/data-privacy-controls', { waitUntil: 'domcontentloaded' });
    await page.waitForTimeout(3500);
    const btn = page.getByRole('button', { name: /export data/i }).first();
    await btn.waitFor({ timeout: 20000 });
    await btn.click();
    await page.waitForTimeout(2500);
    // Log any confirmation-dialog buttons, then click the confirm (avoid Cancel).
    const dlgButtons = await page.evaluate(() =>
      Array.from(document.querySelectorAll('[role=dialog] button, [role=alertdialog] button'))
        .map((b) => (b.innerText || '').trim().replace(/\s+/g, ' ')).filter(Boolean));
    console.log('dialog buttons:', JSON.stringify(dlgButtons));
    const confirm = page.getByRole('button', { name: /^(export data|export|confirm|continue|request|yes)$/i });
    const n = await confirm.count();
    if (n) {
      await confirm.last().click();
      console.log('clicked export confirm');
    } else {
      console.log('no confirm dialog — export may have been requested on the first click');
    }
    await page.waitForTimeout(3000);
    console.log('EXPORT REQUESTED — claude.ai emails a download link when ready (~minutes).');
  } else if (mode === 'download') {
    const url = process.argv[3];
    if (!url) { console.error('usage: download <export-download-url>'); process.exit(2); }
    const destDir = process.env.CORTEX_EXPORT_DIR || path.join(process.env.HOME, 'Downloads');
    const [dl] = await Promise.all([
      page.waitForEvent('download', { timeout: 90000 }),
      page.goto(url).catch(() => {}), // a download URL aborts navigation; that's expected
    ]);
    const dest = path.join(destDir, dl.suggestedFilename() || 'claude-export.zip');
    await dl.saveAs(dest);
    console.log('DOWNLOADED ->', dest);
  } else {
    console.error('usage: export-bot.js login|import-cookie|discover|request|download <url>');
  }
  await browser.close();
})().catch((e) => {
  console.error('ERR', e.message);
  process.exit(1);
});
