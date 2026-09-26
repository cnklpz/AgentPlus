<div align="center">

<img src="docs/images/logo.png" width="88" alt="">

# AgentPlus

Manage providers and model lists for all your coding agents, in one place.

[![Release](https://img.shields.io/github/v/release/cnklpz/AgentPlus?color=2F54EB)](https://github.com/cnklpz/AgentPlus/releases/latest)
[![License](https://img.shields.io/badge/license-AGPL--3.0-blue)](LICENSE)

English | [简体中文](README.zh-CN.md)

</div>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/codex-dark-en.png">
  <img src="docs/images/codex-en.png" alt="AgentPlus main window">
</picture>

Every coding agent stores providers its own way: Codex in TOML, OpenCode in JSONC, Claude Code in environment variables. Switching to a new relay means editing URLs, keys and model lists in half a dozen files.

AgentPlus reads them all. Add a provider once, push it to any agent, and choose which models show up in each agent's picker. Every write is shown as a diff first and backed up, so you can always go back.

It doesn't choose which model you use. You still do that inside each agent.

## Highlights

- **One provider library for 15 agents.** 19 built-in templates for vendors and coding plans; most only need an API key.
- **Per-agent model lists.** Fetch a provider's models, then decide what each picker shows.
- **Review, then apply.** Changes queue up as diffs. Apply them and restart the agent in one click.
- **Backups and rollback.** Original files go to `~/.agentplus/backups/` before every write.
- **Local gateway.** Translates between OpenAI Chat, OpenAI Responses and Anthropic Messages, including streaming and tool calls.
- **Checks.** Measure latency, or send one small request to test a URL, key and model.
- **WSL.** Manage agents inside WSL distros with the same provider library.
- **Privacy mode.** `Ctrl+Shift+H` masks keys, hosts and user names for screenshots and screen sharing.

Also: a command palette (`Ctrl+K`), a tray mode that keeps the gateway running, signed in-app updates, and an English and Chinese UI.

## Install

Download the latest build from [Releases](https://github.com/cnklpz/AgentPlus/releases/latest).

**Windows 10/11 (x64)**: `AgentPlus_<version>_x64-setup.exe`. The installer isn't code-signed yet, so SmartScreen may block the first run. Click *More info → Run anyway*.

**macOS 11+ (experimental)**: `_aarch64.dmg` for Apple silicon, `_x64.dmg` for Intel. The app isn't notarized; allow it under *System Settings → Privacy & Security*. If macOS says it is damaged:

```bash
xattr -cr /Applications/AgentPlus.app
```

Linux builds aren't packaged yet.

AgentPlus checks for updates on startup. Install them from *Settings → General → About*.

## Screenshots

<table>
  <tr>
    <td width="50%"><img src="docs/images/providers-en.png" alt="Provider library"></td>
    <td width="50%"><img src="docs/images/codex-models-en.png" alt="Model list"></td>
  </tr>
  <tr>
    <td align="center">Providers, grouped by relay</td>
    <td align="center">Choosing the models in Codex's picker</td>
  </tr>
  <tr>
    <td colspan="2"><img src="docs/images/gateway-en.png" alt="Local gateway"></td>
  </tr>
  <tr>
    <td colspan="2" align="center">The local gateway, with traffic, latency and failures</td>
  </tr>
</table>

## Supported agents

| Agent | Config |
|---|---|
| Codex (desktop and CLI) | `~/.codex/config.toml`, `models.json` |
| Claude Code | `env` in `~/.claude/settings.json` |
| OpenCode | `~/.config/opencode/opencode.json(c)`, project `opencode.json` |
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
| Trae | Detected only. Its custom models live in your account, so AgentPlus shows the manual steps |

Agents that aren't installed are hidden. If one lives somewhere unusual, set its config folder in *Settings → Agent detection*.

<details>
<summary><b>Codex</b></summary>

- Keeps one fixed provider ID, so old sessions don't disappear from Codex when you switch relays.
- A separate model list per provider, swapped in when you switch.
- Session browser: find out why a session is hidden, move sessions to another provider (with undo), copy the `codex resume` command.
- Health check and cleanup for the database, missing files and oversized logs, with a backup first.
- Import the official model catalog after signing in with ChatGPT once.
- Optional UI patches such as Fast mode and full model names.

</details>

<details>
<summary><b>Claude Code</b></summary>

- Each provider is a profile written to the `env` block of `settings.json`. Relays you set up by hand are detected and can be adopted.
- Choose the model for each role: default, Opus, Sonnet, Haiku and subagents.
- Anthropic protocol only. Relays that speak other protocols can go through the local gateway.
- Toggles for nonessential traffic and the Co-Authored-By line in commits.

</details>

<details>
<summary><b>OpenCode and Kilo Code</b></summary>

- Per-project configs, with settings inherited from the global config marked.
- A form for the common settings: default model, `small_model`, enabled providers, sharing, auto-update, permissions.
- Providers signed in with `opencode auth` are shown read-only.

</details>

<details>
<summary><b>ZCode and MiMo Desktop</b></summary>

- ZCode: model order, per-model context rules, and app settings such as reasoning display, memory and tray.
- MiMo Desktop: the account's built-in models (read-only), skill folders, tray and voice feedback.
- Both can be restarted from AgentPlus.

</details>

<details>
<summary><b>Other CLI agents</b></summary>

- Gemini CLI: multiple profiles, with a warning when environment variables or a project `.env` override them.
- Kimi Code and Hermes: only changed blocks are rewritten; comments and formatting stay.
- CodeBuddy: the IDE and CLI share one config and reload it within a second.
- Droid: the file is re-read right before writing, so changes Droid made in the meantime survive.
- OpenClaw: changing a URL or key also updates the per-agent model files OpenClaw generates.
- pi: only known-valid shapes are written, so one bad field can't break the whole file.
- References like `$VAR`, `${VAR}` and `env_key` are kept as they are.

</details>

## Building from source

AgentPlus is a [Tauri 2](https://tauri.app) app: Rust in `src-tauri/`, React and TypeScript in `src/`. You need Node.js 18+, Rust 1.88+ and the [Tauri prerequisites](https://tauri.app/start/prerequisites/).

```bash
npm install
npm run tauri dev
```

```bash
npm run tauri build                                   # installer
npm run check                                         # frontend types and tests
cd src-tauri && cargo clippy --all-targets && cargo test
```

`npm run dev` runs the UI alone in a browser, with demo data in place of the backend. Conventions, including how UI text is translated, are in [CLAUDE.md](CLAUDE.md).

## License

[AGPL-3.0-only](LICENSE). Copyright (C) 2026 cnklpz.

If you need to use AgentPlus outside the AGPL's terms, such as in a closed-source product, contact the author about a commercial license.
