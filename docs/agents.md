# Supported agents

[English](agents.md) | [简体中文](agents.zh-CN.md)

What AgentPlus reads and writes for each of the 15 agents it recognizes, and what you get per agent. The summary table is in the [README](../README.md#supported-agents).

Only detected agents show up in the app. If one is installed somewhere unusual, point AgentPlus at the folder in *Settings → Agent detection*. Variable references like `$VAR`, `${VAR}` and `env_key` are left as they are in every file it writes.

## Codex

`~/.codex/config.toml` and `models.json`. Desktop app and CLI.

- A fixed provider ID, so past sessions stay visible after you switch API services.
- Model lists are saved per provider and loaded automatically on switch.
- Browse sessions and see why one is hidden. Move a session to another provider (undoable), or copy the `codex resume` command.
- Checks the database, missing files and oversized logs, backing up before any cleanup.
- Import the official model catalog after signing in with ChatGPT.
- Optional UI patches: Fast mode, full model names and a few others.

## Claude Code

The `env` block of `~/.claude/settings.json`.

- A profile per provider, written into `env` when you switch. Configs you wrote by hand can be imported too.
- Set the default model, plus which models Opus, Sonnet, Haiku and subagents use.
- APIs on other protocols go through the local gateway, which converts to and from the Anthropic protocol Claude Code expects.
- Turn off nonessential traffic and the Co-Authored-By line in Git commits.

## OpenCode and Kilo Code

`~/.config/opencode/opencode.json(c)` and project-level `opencode.json`; `~/.config/kilo/kilo.json(c)`.

- Project-level configs, with the settings inherited from the global config marked as inherited.
- A form for the common settings: default model, `small_model`, enabled providers, sharing, auto-update and permissions.
- Providers logged in through `opencode auth` show up read-only.

## ZCode and MiMo Desktop

`~/.zcode/v2/provider_config.json`; `~/.config/mimocode/mimocode.jsonc`.

- ZCode: model order, per-model context rules, reasoning display, memory and tray settings.
- MiMo Desktop: the account's built-in models, skill folders, tray behavior and voice feedback.
- Either app can be restarted from AgentPlus.

## Gemini CLI

`~/.gemini/settings.json`.

- Switch between profiles. You get a warning when environment variables or a project's `.env` override the current settings.

## Qwen Code

`~/.qwen/settings.json`.

- Models are grouped per protocol and endpoint from `modelProviders`. Keys stay as `envKey` references and are written into the `env` block, and Qwen Code hot-reloads the changes.

## Kimi Code and Hermes

`~/.kimi-code/config.toml`; `$HERMES_HOME/config.yaml`.

- Only the changed config blocks are rewritten, comments and formatting intact.

## CodeBuddy

`~/.codebuddy/models.json`.

- The IDE and CLI share one config, hot-reloaded within a second.

## Droid (Factory)

`~/.factory/settings.json`.

- The config is re-read before writing, so changes made while Droid is running survive.

## OpenClaw

`~/.openclaw/openclaw.json`.

- When a URL or key changes, the model files OpenClaw generates for each agent are updated with it.

## pi

`~/.pi/agent/models.json`.

- Configs are written in supported formats, so no invalid field can make the file unreadable.

## Trae

Detection only.

- Custom models live in your account; AgentPlus walks you through adding them by hand.
