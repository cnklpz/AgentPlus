<div align="center">

<img src="docs/images/logo.png" width="88" alt="">

# AgentPlus

在一个地方管理所有编程 Agent 的供应商和模型列表。

[![Release](https://img.shields.io/github/v/release/cnklpz/AgentPlus?color=2F54EB)](https://github.com/cnklpz/AgentPlus/releases/latest)
[![License](https://img.shields.io/badge/license-AGPL--3.0-blue)](LICENSE)

[English](README.md) | 简体中文

</div>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/codex-dark-zh.png">
  <img src="docs/images/codex-zh.png" alt="AgentPlus 主界面">
</picture>

每个编程 Agent 存供应商的方式都不一样：Codex 用 TOML，OpenCode 用 JSONC，Claude Code 用环境变量。换一个中转站，就得在好几个文件里改地址、填密钥、调模型列表。

AgentPlus 把这些配置都读进来。供应商添加一次，就能推送到任意 Agent，再按 Agent 决定选择器里出现哪些模型。每次写入前都先给你看 diff，并备份原文件，随时可以回退。

它不替你选当前用哪个模型，那个仍在各个 Agent 里自己选。

## 功能

- **一个供应商库，15 个 Agent 共用**。内置 19 个厂商和编程套餐模板，大多只要填 API Key。
- **每个 Agent 一份模型列表**。直接拉取供应商的模型，再决定各自的选择器里显示哪些。
- **先看 diff，再写入**。改动先进待应用列表，确认后写入，顺手一键重启 Agent。
- **备份与回滚**。每次写入前，原文件备份到 `~/.agentplus/backups/`。
- **本地网关**。在 OpenAI Chat、OpenAI Responses、Anthropic Messages 之间互转，流式和工具调用都支持。
- **测速与测试**。测延迟，或发一个小请求，检查地址、密钥和模型能不能用。
- **WSL**。管理 WSL 发行版里的 Agent，和 Windows 共用供应商库。
- **隐私模式**。`Ctrl+Shift+H` 遮住密钥、地址和用户名，截图、共享屏幕时用。

另外还有命令面板（`Ctrl+K`）、托盘常驻（网关在后台继续跑）、带签名校验的应用内更新，以及中英文界面。

## 安装

到 [Releases](https://github.com/cnklpz/AgentPlus/releases/latest) 下载最新版本。

**Windows 10/11（x64）**：`AgentPlus_<版本>_x64-setup.exe`。安装包还没有代码签名，首次运行可能被 SmartScreen 拦下，点「更多信息 → 仍要运行」。

**macOS 11+（实验性）**：Apple 芯片下载 `_aarch64.dmg`，Intel 下载 `_x64.dmg`。应用没有经过公证，需要在「系统设置 → 隐私与安全性」里放行。如果提示「已损坏」：

```bash
xattr -cr /Applications/AgentPlus.app
```

Linux 暂时没有打包。

AgentPlus 启动时会检查更新，在「设置 → 通用 → 关于」里安装。

## 截图

<table>
  <tr>
    <td width="50%"><img src="docs/images/providers-zh.png" alt="供应商库"></td>
    <td width="50%"><img src="docs/images/codex-models-zh.png" alt="模型列表"></td>
  </tr>
  <tr>
    <td align="center">按中转站归组的供应商</td>
    <td align="center">挑选 Codex 选择器里的模型</td>
  </tr>
  <tr>
    <td colspan="2"><img src="docs/images/gateway-zh.png" alt="本地网关"></td>
  </tr>
  <tr>
    <td colspan="2" align="center">本地网关的流量、耗时和失败率</td>
  </tr>
</table>

## 支持的 Agent

| Agent | 配置文件 |
|---|---|
| Codex（桌面版和 CLI） | `~/.codex/config.toml`、`models.json` |
| Claude Code | `~/.claude/settings.json` 的 `env` |
| OpenCode | `~/.config/opencode/opencode.json(c)`、项目里的 `opencode.json` |
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
| Trae | 仅识别。自定义模型存在账号里，AgentPlus 给出手动添加的步骤 |

没装的 Agent 不会显示。装在非默认位置的，到「设置 → Agent 识别」里指定配置目录。

<details>
<summary><b>Codex</b></summary>

- 供应商 ID 固定不变，换中转站后历史会话不会从 Codex 里消失。
- 每个供应商一份模型列表，切换时自动换上。
- 会话浏览：查看某个会话为什么被隐藏，把会话迁到另一个供应商（可撤销），复制 `codex resume` 命令。
- 健康检查和清理：数据库、缺失文件、过大的日志，清理前先备份。
- 用 ChatGPT 登录一次，就能导入官方模型目录。
- 可选的界面补丁，如 Fast 模式、显示完整模型名。

</details>

<details>
<summary><b>Claude Code</b></summary>

- 每个供应商是一份配置档，写入 `settings.json` 的 `env`。手动配过的中转也能识别并收编。
- 分别指定默认、Opus、Sonnet、Haiku 和子代理用哪个模型。
- 只支持 Anthropic 协议，其他协议的中转可以经本地网关接入。
- 一键关闭非必要流量、提交里的 Co-Authored-By 署名。

</details>

<details>
<summary><b>OpenCode 和 Kilo Code</b></summary>

- 项目级配置，并标出哪些设置继承自全局。
- 常用设置用表单改：默认模型、`small_model`、启用哪些供应商、分享、自动更新、权限。
- 通过 `opencode auth` 登录的供应商只读显示。

</details>

<details>
<summary><b>ZCode 和 MiMo Desktop</b></summary>

- ZCode：模型顺序、每个模型的上下文规则，以及思考过程显示、记忆、托盘等设置。
- MiMo Desktop：账号内置模型（只读）、技能目录、托盘和语音反馈。
- 两者都能从 AgentPlus 直接重启。

</details>

<details>
<summary><b>其他命令行 Agent</b></summary>

- Gemini CLI：多配置档切换；环境变量或项目 `.env` 覆盖设置时会提醒。
- Kimi Code 和 Hermes：只改变化的配置块，注释和格式保持原样。
- CodeBuddy：IDE 和 CLI 共用一份配置，一秒内热加载。
- Droid：写入前重新读取文件，Droid 运行中写入的改动不会被覆盖。
- OpenClaw：改地址或密钥时，同步更新 OpenClaw 为各 agent 生成的模型文件。
- pi：只写已知有效的结构，一个字段出错不会让整个文件失效。
- `$VAR`、`${VAR}`、`env_key` 这类引用原样保留。

</details>

## 从源码构建

AgentPlus 是一个 [Tauri 2](https://tauri.app) 应用：Rust 在 `src-tauri/`，React 和 TypeScript 在 `src/`。需要 Node.js 18+、Rust 1.88+ 和 [Tauri 的系统依赖](https://tauri.app/start/prerequisites/)。

```bash
npm install
npm run tauri dev
```

```bash
npm run tauri build                                   # 打包安装程序
npm run check                                         # 前端类型检查和测试
cd src-tauri && cargo clippy --all-targets && cargo test
```

`npm run dev` 只在浏览器里跑界面，后端换成演示数据。代码约定（包括界面文案怎么翻译）见 [CLAUDE.md](CLAUDE.md)。

## 许可证

[AGPL-3.0-only](LICENSE)。Copyright (C) 2026 cnklpz。

如需在 AGPL 条款之外使用，比如闭源分发，请联系作者获取商业授权。
