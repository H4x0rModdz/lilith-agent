# Lilith bug-report API

A tiny Cloudflare Worker (free plan) that turns an opt-in bug report from the
Lilith desktop app into a GitHub issue. The app never holds a GitHub token:
it POSTs a fixed-shape JSON to `/v1/report`, and this Worker — holding a
GitHub App key as a secret — opens the issue in `GITHUB_OWNER/GITHUB_REPO`.
Moving reports to a private repository later is just changing that var and
installing the App there; no app release needed.

## What a report contains

Only machine specs and the failure code — no free text, paths, usernames,
conversations or identifiers (see `src/report.js`; unknown fields are
rejected). The issue body is rendered by the Worker, every value inside code
spans, so the endpoint can't be used to post spam, links or @mentions. All
issues are opened by the same bot identity.

## Abuse and quota protection

Cheapest first, so rejected traffic costs nothing:

1. Wrong route/method/headers/size → 404/400/413 before any storage access.
2. Workers rate limiting binding: 3 reports/minute per IP.
3. Body read with a hard 2 KB cap; strict per-field validation.
4. Daily caps in D1: per origin (salted, day-rotating hash of the IP — the IP
   itself is never stored), total reports, and new issues.
5. Same failure (action + code + version) only bumps a counter; the issue gets
   a comment at 5/25/100/500/2500 occurrences.

On the free plan, exceeding a quota never bills — the Worker just stops
answering until the next day. With a domain on Cloudflare, also add a WAF
rate-limiting rule on `/v1/report` and set `workers_dev = false` (see
`wrangler.toml`), so floods are dropped before the Worker even runs.

## Setup

1. **GitHub App** (Settings → Developer settings → GitHub Apps → New):
   no webhook; repository permissions **Issues: Read and write** only
   (Metadata: read is mandatory). Install it on the target repo only. Note the
   App ID and the installation ID (from the installation URL).
2. Generate a private key and convert it to PKCS#8:
   `openssl pkcs8 -topk8 -nocrypt -in app.private-key.pem -out key.pk8.pem`
3. Create the labels `bug` and `auto-report` in the target repo.
4. Cloudflare:
   ```sh
   npm install
   npx wrangler login
   npx wrangler d1 create lilith-reports        # put the id in wrangler.toml
   npx wrangler d1 execute lilith-reports --remote --file schema.sql
   npx wrangler secret put GITHUB_APP_PRIVATE_KEY < key.pk8.pem
   npx wrangler secret put IP_SALT              # any long random string
   npx wrangler deploy
   ```
   Fill `GITHUB_APP_ID` / `GITHUB_INSTALLATION_ID` in `wrangler.toml` first.
5. Put the deployed URL (`https://…/v1/report`) in `DEFAULT_URL` in
   `app/src-tauri/src/report.rs`.

## Local development

```sh
printf 'DRY_RUN=1\nIP_SALT=dev\n' > .dev.vars
npx wrangler d1 execute lilith-reports --local --file schema.sql
npx wrangler dev --port 8787
npm test
```

With `DRY_RUN=1` nothing reaches GitHub; the response carries the issue that
would be created. Point the app at it with `"report_url":
"http://127.0.0.1:8787/v1/report"` in `%APPDATA%\Lilith\config.json`.
