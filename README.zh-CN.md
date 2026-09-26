<div align="center">

<img src="docs/images/logo.png" width="88" alt="">

# AgentPlus

统一管理编程 Agent 的 API 配置和模型列表。

[![Release](https://img.shields.io/github/v/release/cnklpz/AgentPlus?color=2F54EB)](https://github.com/cnklpz/AgentPlus/releases/latest)
[![License](https://img.shields.io/badge/license-AGPL--3.0-blue)](LICENSE)

[English](README.md) | 简体中文

</div>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/codex-dark-zh.png">
  <img src="docs/images/codex-zh.png" alt="AgentPlus 主界面">
</picture>

每个编程 Agent 都有自己的一套配置文件和格式。同时用 Codex、Claude Code、OpenCode、ZCode 这几个工具，换一家 API 服务就得挨个改一遍。AgentPlus 把这些 Agent 的供应商、API Key 和模型列表收到一个桌面应用里管理。

供应商填一次，所有 Agent 都能用；模型列表按 Agent 分别设置。改动会先给出 diff，确认之后才写入，被替换的原文件自动备份。具体用哪个模型，还是你在各个 Agent 里自己决定。

## 安装

从 [Releases](https://github.com/cnklpz/AgentPlus/releases/latest) 下载最新版本。

**Windows 10/11（x64）**：`AgentPlus_<版本>_x64-setup.exe`。安装包还没签名，首次运行时如果 SmartScreen 拦下来，点「更多信息 → 仍要运行」。

**macOS 11+（实验性支持）**：Apple 芯片选 `_aarch64.dmg`，Intel 芯片选 `_x64.dmg`。应用没有公证，需要在「系统设置 → 隐私与安全性」里放行。如果提示「已损坏」，执行：

```bash
xattr -cr /Applications/AgentPlus.app
```

暂不提供 Linux 安装包。更新在启动时检查，可在「设置 → 通用 → 关于」中安装。

## 主要功能

- **供应商**：API 地址和密钥填一次，之后可以挂到任意多个 Agent 上。内置 21 套模板，覆盖常见厂商和编程套餐，多数只要贴一个 Key。
- **模型列表**：从供应商拉取可用模型，再决定每个 Agent 显示哪些。
- 没看过 diff 就不会写入任何文件，原文件备份在 `~/.agentplus/backups/`，之后随时可以恢复。Agent 也能从应用里直接重启。
- **本地网关**：在 OpenAI Chat、OpenAI Responses、Anthropic Messages 之间转换，流式输出和工具调用都支持。
- **连接测试**：测延迟，也发一个真实请求，确认地址、密钥和模型确实能用。
- **WSL**：管理 WSL 发行版里的 Agent，和 Windows 共用同一份供应商库。
- **隐私模式**：`Ctrl+Shift+H` 隐藏密钥、服务地址和用户名，截图或共享屏幕时用得上。

另外还有：命令面板（`Ctrl+K`）、关掉窗口后让网关继续运行的托盘图标、安装前校验签名的应用内更新，以及中英文界面。

<table>
  <tr>
    <td width="50%"><img src="docs/images/providers-zh.png" alt="供应商库，按 API 服务分组"></td>
    <td width="50%"><img src="docs/images/gateway-zh.png" alt="本地网关的流量、耗时和失败率"></td>
  </tr>
</table>

## 支持的 Agent

目前能识别 15 个 Agent，读取和写入的配置文件如下：

| Agent | 配置文件 |
|---|---|
| Codex（桌面版和 CLI） | `~/.codex/config.toml`、`models.json` |
| Claude Code | `~/.claude/settings.json` 中的 `env` |
| OpenCode | `~/.config/opencode/opencode.json(c)`、项目目录下的 `opencode.json` |
| MiMo Desktop | `~/.config/mimocode/mimocode.jsonc` |
| ZCode | `~/.zcode/v2/provider_config.json` |
| Gemini CLI | `~/.gemini/settings.json` |
| Qwen Code | `~/.qwen/settings.json` |
| Kimi Code | `~/.kimi-code/config.toml` |
| Kilo Code | `~/.config/kilo/kilo.json(c)` |
| CodeBuddy | `~/.codebuddy/models.json` |
| Droid（Factory） | `~/.factory/settings.json` |
| Hermes | `$HERMES_HOME/config.yaml` |
| pi | `~/.pi/agent/models.json` |
| OpenClaw | `~/.openclaw/openclaw.json` |
| Trae | 仅支持识别。自定义模型保存在账号里，需要按 AgentPlus 给出的步骤手动添加 |

各 Agent 具体支持什么（会话管理、协议转换、配置合并等）见 **[docs/agents.zh-CN.md](docs/agents.zh-CN.md)**。

## 从源码构建

AgentPlus 基于 [Tauri 2](https://tauri.app)：前端是 `src/` 下的 React + TypeScript，后端是 `src-tauri/` 下的 Rust。需要 Node.js 18+、Rust 1.88+ 和 [Tauri 系统依赖](https://tauri.app/start/prerequisites/)。

安装依赖并启动开发环境：

```bash
npm install
npm run tauri dev
```

打包与检查：

```bash
npm run tauri build    # 生成安装包
npm run check          # 前端类型检查和测试
cd src-tauri && cargo clippy --all-targets && cargo test
```

想在浏览器里预览界面（用的是演示数据）可以跑 `npm run dev`。开发规范和多语言约定见 [CLAUDE.md](CLAUDE.md)。

## 许可证

[AGPL-3.0-only](LICENSE)。Copyright (C) 2026 cnklpz。

如需在 AGPL 条款之外使用（例如闭源分发），请联系作者获取商业授权。
