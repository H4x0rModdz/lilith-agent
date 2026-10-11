# Releasing and auto-update

The app checks GitHub Releases for a newer version every time it starts
(about 12 s after launch) and, if "Update automatically" is on in Settings,
installs it and restarts. Settings also has a manual "Check for updates" button
and the "Start with Windows" switch (enabled once on the first installed run).

## How it works

- `tauri.conf.json` holds the updater public key and the endpoint
  `releases/latest/download/latest.json`.
- Every release carries the signed installer and a `latest.json` pointing to it.
  The app verifies the signature with the public key before installing.
- Debug builds never auto-update or touch the startup entry.

## Publishing a version

1. Bump the version in `Cargo.toml`, `app/src-tauri/tauri.conf.json` and
   `app/package.json`, commit and merge.
2. From the repo root, with `gh` authenticated and the signing key available:

   ```powershell
   .\scripts\release.ps1 -Notes "what changed"
   ```

   It builds the signed installer, writes `latest.json`, tags and creates the
   release.

## Signing key

The private key lives outside the repository (`%USERPROFILE%\.tauri\lilith.key`).
Back it up: if it is lost, installed apps can no longer receive updates and a new
key means a manual reinstall for everyone. Never commit it.

The first installed version that contains the updater must be installed
manually once; from then on updates are automatic.
