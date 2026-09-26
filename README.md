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

If you use Codex, Claude Code, OpenCode or other coding agents, switching API services often means editing each tool's config separately. AgentPlus is a desktop app for managing their providers, API keys and model lists in one place.

Add a provider once, reuse it across agents, and set up a model list for each. Review the config diff before applying changes; the original files are backed up automatically. You still choose which model to use within each agent.

## Features

- **Provider library.** Keep API URLs and keys together and reuse them across agents. Includes 19 templates for API providers and coding plans; most only need an API key.
- **Model lists.** Fetch available models from a provider and choose which ones appear in each agent.
- **Change preview.** Edits stay pending until you review and apply them. Agents can also be restarted with one click.
- **Backups and rollback.** Original files are saved to `~/.agentplus/backups/` before each write and can be restored later.
- **Local gateway.** Converts between OpenAI Chat, OpenAI Responses and Anthropic Messages, with support for streaming and tool calls.
- **Connection tests.** Measure API latency and send a test request to check that the URL, key and model work.
- **WSL support.** Manage agents in WSL distributions using the same provider library as Windows.
- **Privacy mode.** Press `Ctrl+Shift+H` to hide keys, service addresses and usernames when taking screenshots or sharing your screen.

AgentPlus also has a command palette (`Ctrl+K`), system tray support, in-app updates, and an English and Chinese interface. The gateway keeps running while the app is in the tray. Update signatures are verified before installation.

## Install

Download the latest build from [Releases](https://github.com/cnklpz/AgentPlus/releases/latest).

**Windows 10/11 (x64)**: Download `AgentPlus_<version>_x64-setup.exe`. The installer isn't code-signed yet. If SmartScreen prompts you on first launch, click *More info → Run anyway*.

**macOS 11+ (experimental)**: `_aarch64.dmg` for Apple silicon, `_x64.dmg` for Intel. The app isn't notarized; allow it under *System Settings → Privacy & Security*. If macOS says it is damaged:

```bash
xattr -cr /Applications/AgentPlus.app
```

Linux packages are not available yet.

AgentPlus checks for updates on startup. Install them from *Settings → General → About*.

## Screenshots

<table>
  <tr>
    <td width="50%"><img src="docs/images/providers-en.png" alt="Provider library"></td>
    <td width="50%"><img src="docs/images/codex-models-en.png" alt="Model list"></td>
  </tr>
  <tr>
    <td align="center">Manage providers by API service</td>
    <td align="center">Choose which models appear in Codex</td>
  </tr>
  <tr>
    <td colspan="2"><img src="docs/images/gateway-en.png" alt="Local gateway"></td>
  </tr>
  <tr>
    <td colspan="2" align="center">Monitor gateway traffic, request latency and failure rates</td>
  </tr>
</table>

## Supported agents

AgentPlus detects the following 15 agents. The table and notes below describe the support available for each.

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
| Trae | Detection only. Custom models are stored in your account; AgentPlus provides instructions for adding them manually |

Only detected agents appear in the app. For custom installations, specify the config folder in *Settings → Agent detection*.

<details>
<summary><b>Codex</b></summary>

- Use a fixed provider ID to keep past sessions visible when switching API services.
- Save a model list for each provider and load it automatically when switching.
- Browse sessions, see why a session is hidden, move sessions to another provider with undo support, or copy a `codex resume` command.
- Check the database, missing files and oversized logs, with automatic backups before cleanup.
- Import the official model catalog after signing in with ChatGPT.
- Apply optional UI patches for Fast mode, full model names and other settings.

</details>

<details>
<summary><b>Claude Code</b></summary>

- Save a profile for each provider and write it to the `env` block of `settings.json` when switching. Existing manual configurations can be imported too.
- Set the default model and the models used for Opus, Sonnet, Haiku and subagents.
- Connect APIs that use other protocols through the local gateway, which converts requests to and from Claude Code's Anthropic protocol.
- Disable nonessential traffic and Co-Authored-By attribution in Git commits.

</details>

<details>
<summary><b>OpenCode and Kilo Code</b></summary>

- Manage project-level configs and see which settings are inherited from the global config.
- Edit common settings through a form: default model, `small_model`, enabled providers, sharing, auto-update and permissions.
- View providers authenticated through `opencode auth` as read-only entries.

</details>

<details>
<summary><b>ZCode and MiMo Desktop</b></summary>

- ZCode: adjust model order, per-model context rules, reasoning display, memory and tray settings.
- MiMo Desktop: view the account's built-in models and configure skill folders, tray behavior and voice feedback.
- Restart either app directly from AgentPlus.

</details>

<details>
<summary><b>Other agents</b></summary>

- Gemini CLI: switch between profiles, with a warning when environment variables or a project's `.env` override the current settings.
- Kimi Code and Hermes: update only the changed config blocks, preserving comments and formatting.
- CodeBuddy: share one config between the IDE and CLI, with hot reload within a second.
- Droid: re-read the config before writing to preserve changes made by Droid while it is running.
- OpenClaw: update the model files OpenClaw generates for each agent when changing a URL or key.
- pi: write configs in supported formats to avoid invalid fields that could make the file unreadable.
- Preserve variable references such as `$VAR`, `${VAR}` and `env_key`.

</details>

## Building from source

AgentPlus uses [Tauri 2](https://tauri.app), with a React and TypeScript frontend in `src/` and a Rust backend in `src-tauri/`. Install Node.js 18+, Rust 1.88+ and the [Tauri prerequisites](https://tauri.app/start/prerequisites/) before building.

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

To preview the interface in a browser with demo data, run `npm run dev`. See [CLAUDE.md](CLAUDE.md) for development and translation conventions.

## License

[AGPL-3.0-only](LICENSE). Copyright (C) 2026 cnklpz.

For uses outside the AGPL's terms, such as closed-source distribution, contact the author for a commercial license.
