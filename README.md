<div align="center">

<img src="docs/images/logo.png" width="88" alt="">

# AgentPlus

Manage API settings and model lists across your coding agents.

[![Release](https://img.shields.io/github/v/release/cnklpz/AgentPlus?color=2F54EB)](https://github.com/cnklpz/AgentPlus/releases/latest)
[![License](https://img.shields.io/badge/license-AGPL--3.0-blue)](LICENSE)

English | [简体中文](README.zh-CN.md)

</div>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/codex-dark-en.png">
  <img src="docs/images/codex-en.png" alt="AgentPlus main window">
</picture>

Every coding agent keeps its own config file, in its own format. Use more than one — Codex, Claude Code, OpenCode, ZCode — and adding an API provider means editing each of them by hand. AgentPlus is a desktop app that keeps those providers, keys and model lists in one place.

Add a provider once and any agent can use it. Model lists are set per agent, and edits are shown as a diff before anything is written; the file being replaced is backed up first. Which model to run is still your call inside each agent.

## Features

- **Providers** — an API URL and key go in once, then attach to as many agents as you like. 21 built-in templates cover the common providers and coding plans; most need nothing but a key.
- **Model lists** — fetch the available models from a provider, then pick what each agent shows.
- Nothing is written until you've reviewed the diff. Originals go to `~/.agentplus/backups/` and can be restored later, and agents restart from the app.
- **Local gateway** — translates between OpenAI Chat, OpenAI Responses and Anthropic Messages, streaming and tool calls included.
- **Connection tests** — latency, plus a real request to confirm the URL, key and model work.
- **WSL** — agents inside a WSL distro share the same provider library as Windows.
- **Privacy mode** — `Ctrl+Shift+H` hides keys, service URLs and usernames for screenshots and screen sharing.

Also: a command palette (`Ctrl+K`), a tray icon that keeps the gateway running after the window is closed, in-app updates that verify signatures before installing, and an English or Chinese interface.

## Install

Download the latest build from [Releases](https://github.com/cnklpz/AgentPlus/releases/latest).

**Windows 10/11 (x64)** — `AgentPlus_<version>_x64-setup.exe`. The installer isn't code-signed yet; if SmartScreen complains on first launch, click *More info → Run anyway*.

**macOS 11+ (experimental)** — `_aarch64.dmg` for Apple silicon, `_x64.dmg` for Intel. The app isn't notarized, so allow it under *System Settings → Privacy & Security*. If macOS says it's damaged:

```bash
xattr -cr /Applications/AgentPlus.app
```

No Linux packages yet.

Updates are checked at startup and installed from *Settings → General → About*.

## Screenshots

<table>
  <tr>
    <td width="50%"><img src="docs/images/providers-en.png" alt="Provider library"></td>
    <td width="50%"><img src="docs/images/codex-models-en.png" alt="Model list"></td>
  </tr>
  <tr>
    <td align="center">Providers, grouped by API service</td>
    <td align="center">Pick which models Codex shows</td>
  </tr>
  <tr>
    <td colspan="2"><img src="docs/images/gateway-en.png" alt="Local gateway"></td>
  </tr>
  <tr>
    <td colspan="2" align="center">Gateway traffic, request latency and failure rates</td>
  </tr>
</table>

## Supported agents

AgentPlus recognizes these 15 agents:

| Agent | Config |
|---|---|
| Codex (desktop and CLI) | `~/.codex/config.toml`, `models.json` |
| Claude Code | `env` in `~/.claude/settings.json` |
| OpenCode | `~/.config/opencode/opencode.json(c)`, project-level `opencode.json` |
| MiMo Desktop | `~/.config/mimocode/mimocode.jsonc` |
| ZCode | `~/.zcode/v2/provider_config.json` |
| Gemini CLI | `~/.gemini/settings.json` |
| Qwen Code | `~/.qwen/settings.json` |
| Kimi Code | `~/.kimi-code/config.toml` |
| Kilo Code | `~/.config/kilo/kilo.json(c)` |
| CodeBuddy | `~/.codebuddy/models.json` |
| Droid (Factory) | `~/.factory/settings.json` |
| Hermes | `$HERMES_HOME/config.yaml` |
| pi | `~/.pi/agent/models.json` |
| OpenClaw | `~/.openclaw/openclaw.json` |
| Trae | Detection only. Custom models live in your account; AgentPlus walks you through adding them by hand |

Only what it detects on disk shows up in the app. Installed somewhere unusual? Point AgentPlus at the folder in *Settings → Agent detection*.

<details>
<summary><b>Codex</b></summary>

- A fixed provider ID, so past sessions stay visible after you switch API services.
- Model lists are saved per provider and loaded automatically on switch.
- Browse sessions and see why one is hidden. Move a session to another provider (undoable), or copy the `codex resume` command.
- Checks the database, missing files and oversized logs, backing up before any cleanup.
- Import the official model catalog after signing in with ChatGPT.
- Optional UI patches: Fast mode, full model names and a few others.

</details>

<details>
<summary><b>Claude Code</b></summary>

- A profile per provider, written into the `env` block of `settings.json` when you switch. Configs you wrote by hand can be imported too.
- Set the default model, plus which models Opus, Sonnet, Haiku and subagents use.
- APIs on other protocols go through the local gateway, which converts to and from the Anthropic protocol Claude Code expects.
- Turn off nonessential traffic and the Co-Authored-By line in Git commits.

</details>

<details>
<summary><b>OpenCode and Kilo Code</b></summary>

- Project-level configs, with the settings inherited from the global config marked as inherited.
- A form for the common settings: default model, `small_model`, enabled providers, sharing, auto-update and permissions.
- Providers logged in through `opencode auth` show up read-only.

</details>

<details>
<summary><b>ZCode and MiMo Desktop</b></summary>

- ZCode: model order, per-model context rules, reasoning display, memory and tray settings.
- MiMo Desktop: the account's built-in models, skill folders, tray behavior and voice feedback.
- Either app can be restarted from AgentPlus.

</details>

<details>
<summary><b>Other agents</b></summary>

- Gemini CLI: switch between profiles. You get a warning when environment variables or a project's `.env` override the current settings.
- Kimi Code and Hermes: only the changed config blocks are rewritten, comments and formatting intact.
- CodeBuddy: the IDE and CLI share one config, hot-reloaded within a second.
- Droid: the config is re-read before writing, so changes made while Droid is running survive.
- OpenClaw: when a URL or key changes, the model files OpenClaw generates for each agent are updated with it.
- pi: configs are written in supported formats, so no invalid field can make the file unreadable.
- Variable references like `$VAR`, `${VAR}` and `env_key` are left alone.

</details>

## Building from source

AgentPlus is built on [Tauri 2](https://tauri.app): React and TypeScript in `src/`, Rust in `src-tauri/`. You'll need Node.js 18+, Rust 1.88+ and the [Tauri prerequisites](https://tauri.app/start/prerequisites/).

Install dependencies and start the development app:

```bash
npm install
npm run tauri dev
```

Build and run checks:

```bash
npm run tauri build    # Build the installer
npm run check          # Frontend type checks and tests
cd src-tauri && cargo clippy --all-targets && cargo test
```

To preview the interface in a browser with demo data, run `npm run dev`. Development and translation conventions are in [CLAUDE.md](CLAUDE.md).

## License

[AGPL-3.0-only](LICENSE). Copyright (C) 2026 cnklpz.

For uses outside the AGPL's terms, such as closed-source distribution, contact the author for a commercial license.
