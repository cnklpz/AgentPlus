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

Each agent stores its API settings in its own way: Codex in `config.toml`, Claude Code in the `env` block of `settings.json`, OpenCode in its own jsonc. Add one provider and you're editing several files by hand, each in a different format. AgentPlus is a desktop app that keeps providers, keys and model lists in one place, and writes those files for you.

Fill in a provider once, then attach it to whichever agents you want. Model lists are per agent. You see the diff before anything is written, and the file being replaced is backed up in case you want it back. Which model to run is still your call inside each agent.

## Features

- **Provider library.** Put an API URL and key in once and every agent can use it. There are 21 templates for common providers and coding plans; most need nothing from you but a key.
- **Model lists.** Fetch what a provider offers, then choose what each agent shows.
- **Change preview.** Nothing is written until you've looked at the diff, and whatever gets replaced is saved to `~/.agentplus/backups/` first.
- **Local gateway.** Converts between OpenAI Chat, OpenAI Responses and Anthropic Messages, streaming and tool calls included — which is how Claude Code can use a provider that only speaks OpenAI.
- **Connection tests.** Latency, plus a real request, so you know the URL, key and model work before you switch.
- **WSL.** Agents inside a WSL distro share the same provider library as Windows.
- **Privacy mode.** `Ctrl+Shift+H` hides keys, service URLs and usernames before a screenshot or a screen share.

Agents can also be restarted from the app. Then there's the command palette (`Ctrl+K`), a tray icon that keeps the gateway running after you close the window, in-app updates that check their own signature, and the interface in English or Chinese.

## Install

Download the latest build from [Releases](https://github.com/cnklpz/AgentPlus/releases/latest).

**Windows 10/11 (x64)** — `AgentPlus_<version>_x64-setup.exe`. The installer isn't code-signed yet, so SmartScreen may warn you on first launch; click *More info → Run anyway*.

**macOS 11+ (experimental)** — `_aarch64.dmg` for Apple silicon, `_x64.dmg` for Intel. The app isn't notarized, so allow it under *System Settings → Privacy & Security*. If macOS insists it's damaged:

```bash
xattr -cr /Applications/AgentPlus.app
```

Linux packages aren't available yet.

Updates are checked on startup and installed from *Settings → General → About*.

## Screenshots

<table>
  <tr>
    <td width="50%"><img src="docs/images/providers-en.png" alt="Provider library"></td>
    <td width="50%"><img src="docs/images/codex-models-en.png" alt="Model list"></td>
  </tr>
  <tr>
    <td align="center">The provider library, grouped by API service</td>
    <td align="center">Choosing which models Codex shows</td>
  </tr>
  <tr>
    <td colspan="2"><img src="docs/images/gateway-en.png" alt="Local gateway"></td>
  </tr>
  <tr>
    <td colspan="2" align="center">Gateway traffic, latency and failure rates</td>
  </tr>
</table>

## Supported agents

AgentPlus recognizes 15 agents and reads and writes the following config files:

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

Only the agents it finds show up in the app. If yours is installed somewhere unusual, point AgentPlus at the folder in *Settings → Agent detection*.

<details>
<summary><b>Codex</b></summary>

- Use a fixed provider ID so past sessions stay visible when you switch API services.
- Save a model list per provider; switching loads it automatically.
- Browse sessions and see why one is hidden. Move a session to another provider (undoable), or copy its `codex resume` command.
- Checks the database, missing files and oversized logs, and backs up before cleaning up.
- Import the official model catalog once you've signed in with ChatGPT.
- Optional UI patches: Fast mode, full model names and a couple of others.

</details>

<details>
<summary><b>Claude Code</b></summary>

- Save a profile per provider; switching writes it into the `env` block of `settings.json`. Configs you wrote by hand can be imported too.
- Set the default model, and which models Opus, Sonnet, Haiku and subagents use.
- APIs on other protocols go through the local gateway, which converts to and from the Anthropic protocol Claude Code expects.
- Turn off nonessential traffic and the Co-Authored-By line in Git commits.

</details>

<details>
<summary><b>OpenCode and Kilo Code</b></summary>

- Manage project-level configs, and see which settings come from the global config.
- Edit the common settings in a form: default model, `small_model`, enabled providers, sharing, auto-update, permissions.
- Providers you logged in with `opencode auth` show up read-only.

</details>

<details>
<summary><b>ZCode and MiMo Desktop</b></summary>

- ZCode: model order, per-model context rules, reasoning display, memory, tray.
- MiMo Desktop: the models built into your account, skill folders, tray behavior, voice feedback.
- Restart either app from AgentPlus.

</details>

<details>
<summary><b>Other agents</b></summary>

- Gemini CLI: switch between profiles. If environment variables or a project's `.env` override the settings, you get a warning.
- Kimi Code and Hermes: only the blocks that changed are rewritten, comments and formatting left intact.
- CodeBuddy: one config for the IDE and the CLI, hot-reloaded within a second.
- Droid: the config is re-read before writing, so edits made while Droid is running aren't lost.
- OpenClaw: change a URL or key and the model files it generates per agent are updated too.
- pi: written in the formats pi supports, so an invalid field can't make the file unreadable.
- Variable references (`$VAR`, `${VAR}`, `env_key`) are left alone.

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
