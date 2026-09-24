# AgentPlus

[简体中文](README.md) | English

A small desktop app for managing the **providers (API base URL + key) and model lists** of AI coding agents in one place.

Every agent stores its configuration differently: Codex uses TOML plus `models.json`, OpenCode uses JSONC, Claude Code uses environment variables in `settings.json`…
AgentPlus reads all of them and lets you edit them in one UI: maintain a provider once on the Providers page, push it to any agent,
then decide per agent which models show up in its model picker.

> AgentPlus manages *which providers exist and which models are in the picker*. It **does not choose the model you are using**; you still do that in each agent.

## Download

Get `AgentPlus_<version>_x64-setup.exe` from [Releases](https://github.com/cnklpz/AgentPlus/releases/latest) and run it.

- Requirements: Windows 10 / 11 (x64). WebView2 ships with Windows 11; on Windows 10 the installer adds it if needed.
- The installer is not code-signed, so SmartScreen may say "Windows protected your PC" on first run. Click "More info → Run anyway".
- **macOS and Linux are not supported yet.** The code compiles, but agent detection, restarting agents and opening folders are only implemented for Windows so far.

### Updates

At startup AgentPlus checks GitHub Releases for a new version. When there is one, a small dot appears on the Settings button in the top-right corner.
Open Settings → General → About to read the release notes, then click "Download and install": AgentPlus downloads the update, verifies its signature, installs it and reopens.

To stop the startup check, turn off "Check for updates at startup" in the same place and use "Check for updates" when you want to.

## Supported agents

| Agent | Configuration it manages |
|---|---|
| Codex (desktop + CLI) | `~/.codex/config.toml`, model catalog `models.json`, keys in `~/.codex/.env` |
| Claude Code | Providers as profiles; switching writes the `env` block of `~/.claude/settings.json` |
| OpenCode | `~/.config/opencode/opencode.json(c)`, keys in `auth.json`; per-project configs too |
| MiMo Desktop | `~/.config/mimocode/mimocode.jsonc` |
| ZCode | `~/.zcode/v2/provider_config.json` |
| Gemini CLI | `~/.gemini/.env` + `settings.json` |
| Qwen Code | `~/.qwen/settings.json` |
| Kimi Code | `~/.kimi-code/config.toml` |
| Kilo Code | `~/.config/kilo/kilo.json(c)` |
| CodeBuddy | `~/.codebuddy/models.json` |
| Droid (Factory) | `~/.factory/settings.json` |
| Hermes | `config.yaml` under HERMES_HOME |
| pi | `~/.pi/agent/models.json` |
| OpenClaw | `~/.openclaw/openclaw.json` |
| Trae | Detection only (custom models live in the account's cloud storage); shows manual steps |

Agents that aren't installed are hidden. For agents installed in a non-default location, set the config folder in Settings → Agent detection;
you can also hide detected agents you don't use from the sidebar there.

## Features

- **Provider library**: all providers in one place; add, edit or remove them and push the change to the agents you pick. Templates for common vendors and coding plans only need an API key.
  Fetch a provider's model list, measure latency, or send one small real request to test it.
- **Model lists**: choose per agent which models appear in its picker, with fields such as the context window.
- **Preview, then apply**: every change goes into a pending list where you can review each diff before it is written; afterwards, restart the agent in one click so it picks up the change.
- **History and rollback**: the original files are backed up to `~/.agentplus/backups/` before every write, and can be restored in one click.
- **Local gateway**: a small HTTP server on `127.0.0.1` that converts between OpenAI Chat Completions, OpenAI Responses and Anthropic Messages on the fly (streaming included).
  It lets Codex (Responses only) and Claude Code (Anthropic only) use relays that speak another protocol. It has an error breaker, and each agent gets its own gateway key.
- **WSL**: switch the target environment between Windows and WSL distros; the provider library is shared between them.
- **Codex tools**: session browser, health check, safe cleanup and provider repair; fetch the official model list; UI patches for Fast mode and full model names.
- **OpenCode project configs**: give a single project folder its own providers, default model and permissions, merged with the global config the way OpenCode does it.
- **Tray and privacy mode**: closing the window can minimize it to the tray while the gateway keeps running; privacy mode (`Ctrl+Shift+H`) masks keys, hosts and user names for screenshots and screen sharing.
- **Command palette**: `Ctrl+K` searches providers, models, settings and sessions.
- **In-app updates**: see [Updates](#updates).
- **Multi-device sync** (not available yet): export and import providers and model lists through a shared folder, without API keys.

The UI is available in Simplified Chinese and English. Switch in Settings → Interface → Language; it follows the system by default.

## Development

Stack: [Tauri 2](https://tauri.app) (Rust, `src-tauri/`) + React 18 + TypeScript (`src/`) + Vite.

Prerequisites: Node.js 18+, stable Rust (1.88 or newer) and [Tauri's system dependencies](https://tauri.app/start/prerequisites/) (WebView2 and the MSVC build tools on Windows).

```bash
npm install
npm run tauri dev      # run in development mode
npm run tauri build    # build the installer (no signing key needed locally, see below)
```

For UI-only work, `npm run dev` previews the app in a browser with backend calls replaced by local demo data.

Checks and tests:

```bash
npm run check                                   # frontend: type check + unit tests (vitest)
cd src-tauri && cargo clippy --all-targets && cargo test   # backend: lint + unit tests
```

Tests marked `#[ignore]` in `cargo test` read the real agent configs on this machine (read-only). Run one with
`cargo test <name> -- --ignored --nocapture`.

### Layout

```
src/                    frontend
  components/           pages and components
  i18n/zh, i18n/en      UI text (Chinese is the source language)
  api.ts                calls into the backend
  updater.ts            in-app update state
src-tauri/src/          backend
  adapters/             one adapter per agent, reads and writes its config files
  gateway/              local gateway: protocol conversion, HTTP server, breaker, keys
  sessions.rs           Codex sessions
  history.rs            backups and rollback
  update.rs             in-app updates (tauri-plugin-updater)
  i18n.rs               backend text in both languages
scripts/                small release scripts
docs/design.html        design notes
```

AgentPlus keeps its own data in `~/.agentplus/` (`store.json` and `backups/`).

Language and coding conventions are in [CLAUDE.md](CLAUDE.md): no hard-coded UI text, and every new string needs both Chinese and English.

## Releasing

Releases are built by GitHub Actions ([`.github/workflows/release.yml`](.github/workflows/release.yml)). Pushing a `v*` tag builds on Windows,
signs the installer with the private key and creates a **draft** release with the installer, its signature and the `latest.json` used by the updater.

**One-time setup**

1. The repository must be public (releases of a private repository can't be read without signing in, so updates would fail).
2. Signing key: the public key is in `plugins.updater.pubkey` in `src-tauri/tauri.conf.json`.
   The private key **stays on the maintainer's machine**; add its contents as the Actions secret `TAURI_SIGNING_PRIVATE_KEY`
   (Settings → Secrets and variables → Actions). If the key has a password, also add `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.

   > If the private key is lost, installed copies can never receive updates again (users would have to download a new installer by hand). Back it up.

**Every release**

1. Add a `## 0.2.0` section at the top of [CHANGELOG.md](CHANGELOG.md) with notes in Chinese and English, and commit it.
2. Bump the version and tag it (the working tree must be clean):

   ```bash
   npm version 0.2.0      # syncs package.json, tauri.conf.json, Cargo.toml and Cargo.lock, commits and tags v0.2.0
   git push --follow-tags
   ```

3. Wait for the Release workflow in Actions to finish (about 10 minutes), check the draft on the Releases page, then click **Publish release**.
   Installed copies of AgentPlus see the new version at their next startup or manual check.

Released the wrong thing? Turn the release back into a draft or delete it; clients only look at the latest published release.

## Contributing

Bug reports and suggestions are welcome in [Issues](https://github.com/cnklpz/AgentPlus/issues).

Code contributions (pull requests) are not accepted for now. When they are, contributors will need to sign a Contributor License Agreement (CLA)
allowing the author to use contributed code under terms other than the AGPLv3, including commercial licenses.

## License

Copyright (C) 2026 cnklpz

AgentPlus is released under the [GNU Affero General Public License v3.0](LICENSE) (SPDX: `AGPL-3.0-only`).
If you modify and distribute it, or offer a modified version to others over a network, you must make the corresponding source code available under the AGPLv3.

For use outside the AGPLv3 terms (for example closed-source distribution), contact the author for a commercial license.
