# Lilith Agent

**A local-first AI companion that lives in the corner of your Windows desktop and actually does things.**

Lilith starts as a small orb in the corner of your screen. Click it to chat, and she can open apps, find and
organize files, download things, change your wallpaper, adjust volume, search the web, or hand a coding job
to Claude Code or Codex in the background. You stay in control: every risky action goes through an approval
prompt, a kill switch stops her instantly, and everything she does is logged.

Everything runs **on your machine and costs nothing**. The language model is served by
[Ollama](https://ollama.com), so no cloud API is used and no keys are needed.

> **Status:** early MVP (phase 1 of 5). The 2D orb, chat, guardrails, Windows actions and agent delegation
> work today. The 3D avatar, voice, screen vision with her own pink cursor, and phone access are next. See
> the [roadmap](#roadmap).

## Features

- **Orb + chat:** an always-on-top orb in the corner. Click to chat, drag to move it, and double-click to
  summon the 3D form (coming in phase 2).
- **Windows actions:** open/close apps (Start Menu and Store apps included), open/find/list/move/copy files,
  send files to the Recycle Bin, download files, set the wallpaper (from a path, a URL, or just a description
  like *"black cat"*), set volume and mute, read and write the clipboard, lock/sleep/restart, web search, and
  read a page.
- **Multi-step reasoning:** she chains tools one at a time and reacts to each result, for example *"find the
  official VLC site and open it"*.
- **Agent delegation:** *"ask Claude to fix the build in my project"* sends a self-contained prompt to a
  coding agent CLI (Claude Code, Codex, or any CLI you configure). The job runs in the background, and you
  follow its live log in **Lilith's panel**, opened from the tray icon under *Show hidden icons*.
- **Guardrails:**
  - Every action maps to a permission scope. Safe scopes run directly; dangerous ones (delete, download,
    clipboard read, power, delegation, …) ask first with *Deny / Allow once / Always*, and deny automatically
    after 30 s. Unknown actions are always denied.
  - Rules are per channel (local today, Discord later) and you decide them.
  - `Ctrl+Alt+Esc` is a global kill switch.
  - Every decision and result is appended to an audit log.
- **Light by design:** Tauri 2 (Rust and the system WebView2) instead of Electron. Idle cost is close to zero.
  The default model (`qwen3.5:4b`, about 3.3 GB of VRAM) answers simple commands in under a second on a
  mid-range GPU.

## Requirements

- Windows 10/11
- [Ollama](https://ollama.com) with a tool-calling model:

  ```powershell
  ollama pull qwen3.5:4b
  ```

- [Rust](https://rustup.rs) (stable) and [Node.js](https://nodejs.org) 22+
- Optional, for delegation: [Claude Code](https://docs.claude.com/en/docs/claude-code) and/or
  [Codex CLI](https://github.com/openai/codex) on your `PATH`

## Getting started

```powershell
git clone https://github.com/H4x0rModdz/lilith-agent.git
cd lilith-agent\app
npm install
npm run tauri dev
```

The orb appears in the bottom-right corner. To build an installer, run `npm run tauri build`; the NSIS
output goes to `target\release\bundle`.

## Configuration

Lilith keeps her files in `%APPDATA%\Lilith\`:

| File | Purpose |
|---|---|
| `config.json` | Model, Ollama host, context size, and the agent CLIs available for delegation |
| `permissions.json` | Your per-channel rules: `{"channels": {"local": {"filesystem.delete": "allow"}}}` (`allow` / `ask` / `deny`) |
| `logs\audit.jsonl` | One line per action: channel, params, scope, outcome |
| `avatar.png` / `.jpg` / `.webp` | Optional picture for Lilith in the chat and panel (max 2 MB). It isn't shipped, so you can pick your own. |

Rules match by dotted prefix, and the most specific rule wins. For example, `filesystem` covers
`filesystem.write.downloads`. You can also set the environment variables `LILITH_MODEL` and `OLLAMA_HOST`;
a bind-style value such as `0.0.0.0:11434` is normalized automatically.

To add another coding agent, add an entry under `agents` in `config.json`. The prompt is always sent through
**stdin**, never on the command line, so model-written text can't inject shell commands:

```json
"agents": {
  "my-agent": { "command": "my-agent-cli", "args": ["run", "--json"], "format": "plain" }
}
```

## Architecture

```
app/src-tauri   Tauri shell: windows (orb, chat, panel), tray, approvals, kill switch
crates/
  lilith-brain  Ollama client (streaming + tool calling), multi-step agent loop, memory, emotion
  lilith-guard  Scopes, per-channel policy, audit log
  lilith-win    Windows actions (Win32, Shell, Core Audio, UI Automation later)
  lilith-agents Background jobs for coding-agent CLIs, with live log parsing
docs/           Architecture, design decisions (ADRs), future features
```

The brain and the guardrails were ported from [jarvis-os](https://github.com/H4x0rModdz/jarvis-os), an
AI-native Linux desktop by the same author. See [docs/architecture.md](docs/architecture.md) and
[docs/decisions](docs/decisions) for the reasoning behind each choice.

## Roadmap

1. **MVP:** orb, chat, Windows actions, guardrails, agent delegation. *(done)*
2. **3D form:** VRM avatar (three-vrm) with facial expressions, local voice (whisper.cpp + Piper/Kokoro),
   and lip-sync.
3. **Eyes and hands:** event-driven, low-cost screen perception (UI Automation → OCR → vision model only on
   demand), plus her own pink cursor on an overlay.
4. **Remote:** a Discord bot and live screen streaming to your phone over Tailscale.
5. **Extras:** a parallel desktop where she works without sharing your cursor
   ([notes](docs/new-features/parallel-desktop.md)), plus WhatsApp and calls.

## Development

```powershell
cargo test --workspace          # unit tests (brain, guard, windows actions, agents, wiring)
cargo run -p lilith-brain --example ask -- "open spotify"   # talk to the model with simulated actions
cargo run -p lilith-win --example smoke                     # read-only checks against your real system
```

## License

[GPL-3.0](LICENSE), the same license as jarvis-os, which the brain and the guardrails were ported from.
