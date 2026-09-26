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

同时使用 Codex、Claude Code、OpenCode 等工具时，更换 API 服务往往要逐个修改配置文件。AgentPlus 是一个桌面配置工具，可以集中管理这些 Agent 的供应商、API Key 和模型列表。

添加一次供应商，就能复用到多个 Agent，并分别设置各自的模型列表。修改配置时可以先预览差异，确认后再写入，原文件会自动备份。使用哪个模型，仍由你在各个 Agent 中选择。

## 主要功能

- **供应商管理**：统一维护 API 地址和密钥，供多个 Agent 复用。内置 19 套配置模板，覆盖常见厂商和编程套餐，大多只需填写 API Key。
- **模型管理**：获取供应商的模型列表，为每个 Agent 设置要显示的模型。
- **改动预览**：修改先暂存，确认差异后再写入配置，支持一键重启 Agent。
- **备份与回滚**：写入前将原文件备份到 `~/.agentplus/backups/`，需要时可恢复。
- **本地网关**：支持 OpenAI Chat、OpenAI Responses、Anthropic Messages 协议转换，兼容流式输出和工具调用。
- **连接测试**：测试接口延迟，发送请求检查 API 地址、密钥和模型是否可用。
- **WSL 支持**：管理 WSL 发行版中的 Agent，与 Windows 共用供应商库。
- **隐私模式**：按 `Ctrl+Shift+H` 隐藏密钥、服务地址和用户名，方便截图或共享屏幕。

此外还支持命令面板（`Ctrl+K`）、托盘常驻、应用内更新和中英文界面。最小化到托盘后，网关会继续运行；更新包会在安装前校验签名。

## 安装

从 [Releases](https://github.com/cnklpz/AgentPlus/releases/latest) 下载最新版本。

**Windows 10/11（x64）**：下载 `AgentPlus_<版本>_x64-setup.exe`。安装包暂未签名，首次运行时如遇 SmartScreen 提示，点击「更多信息 → 仍要运行」。

**macOS 11+（实验性支持）**：Apple 芯片选择 `_aarch64.dmg`，Intel 芯片选择 `_x64.dmg`。应用暂未经过公证，需要在「系统设置 → 隐私与安全性」中允许运行。若提示应用「已损坏」，可执行：

```bash
xattr -cr /Applications/AgentPlus.app
```

暂不提供 Linux 安装包。

AgentPlus 会在启动时检查更新，可在「设置 → 通用 → 关于」中安装新版本。

## 截图

<table>
  <tr>
    <td width="50%"><img src="docs/images/providers-zh.png" alt="供应商库"></td>
    <td width="50%"><img src="docs/images/codex-models-zh.png" alt="模型列表"></td>
  </tr>
  <tr>
    <td align="center">按中转站分组管理供应商</td>
    <td align="center">设置 Codex 中显示的模型</td>
  </tr>
  <tr>
    <td colspan="2"><img src="docs/images/gateway-zh.png" alt="本地网关"></td>
  </tr>
  <tr>
    <td colspan="2" align="center">查看网关流量、请求耗时和失败率</td>
  </tr>
</table>

## 支持的 Agent

目前可识别以下 15 个 Agent，具体支持范围见下表及后面的说明。

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
| Trae | 仅支持识别。自定义模型保存在账号中，需按 AgentPlus 提供的步骤手动添加 |

列表只显示已检测到的 Agent。如果使用了自定义安装位置，可在「设置 → Agent 识别」中指定配置目录。

<details>
<summary><b>Codex</b></summary>

- 支持固定供应商 ID，避免切换中转站后看不到历史会话。
- 可为每个供应商保存独立的模型列表，切换供应商时自动加载。
- 浏览历史会话、查看会话被隐藏的原因、迁移会话到其他供应商，也可复制 `codex resume` 命令。会话迁移支持撤销。
- 检查数据库、缺失文件和过大的日志，清理前自动备份。
- 通过 ChatGPT 登录后，可导入官方模型目录。
- 提供 Fast 模式、显示完整模型名等可选界面补丁。

</details>

<details>
<summary><b>Claude Code</b></summary>

- 为每个供应商保存一套配置，切换时写入 `settings.json` 的 `env`。已有的手动配置也可导入管理。
- 设置默认模型，以及 Opus、Sonnet、Haiku 和子代理使用的模型。
- Claude Code 使用 Anthropic 协议，其他协议的 API 服务可通过本地网关接入。
- 可关闭非必要流量，以及 Git 提交中的 Co-Authored-By 署名。

</details>

<details>
<summary><b>OpenCode 和 Kilo Code</b></summary>

- 支持项目级配置，并标注从全局配置继承的设置。
- 通过表单调整默认模型、`small_model`、启用的供应商、分享、自动更新和权限等常用设置。
- 通过 `opencode auth` 登录的供应商仅供查看。

</details>

<details>
<summary><b>ZCode 和 MiMo Desktop</b></summary>

- ZCode：调整模型顺序和各模型的上下文规则，设置思考过程显示、记忆、托盘等选项。
- MiMo Desktop：查看账号内置模型，设置技能目录、托盘和语音反馈。
- 两者均支持在 AgentPlus 中直接重启。

</details>

<details>
<summary><b>其他 Agent</b></summary>

- Gemini CLI：支持多套配置切换。当环境变量或项目 `.env` 覆盖当前设置时，会显示提醒。
- Kimi Code 和 Hermes：仅更新有改动的配置块，保留原有注释和格式。
- CodeBuddy：IDE 和 CLI 共用配置，修改后可在一秒内热加载。
- Droid：写入前重新读取配置，保留 Droid 运行期间对文件的修改。
- OpenClaw：修改地址或密钥时，同步更新 OpenClaw 为各 agent 生成的模型文件。
- pi：按支持的格式写入配置，避免无效字段导致整个文件无法读取。
- 保留配置中的 `$VAR`、`${VAR}`、`env_key` 等变量引用。

</details>

## 从源码构建

AgentPlus 基于 [Tauri 2](https://tauri.app) 开发，前端使用 React 和 TypeScript（`src/`），后端使用 Rust（`src-tauri/`）。构建前需安装 Node.js 18+、Rust 1.88+ 和 [Tauri 系统依赖](https://tauri.app/start/prerequisites/)。

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

如需在浏览器中预览界面，可运行 `npm run dev`，此时使用演示数据。开发规范和多语言约定见 [CLAUDE.md](CLAUDE.md)。

## 许可证

[AGPL-3.0-only](LICENSE)。Copyright (C) 2026 cnklpz。

如需在 AGPL 条款之外使用（例如闭源分发），请联系作者获取商业授权。
