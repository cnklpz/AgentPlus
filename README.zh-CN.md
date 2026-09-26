<div align="center">

<img src="docs/images/logo.png" width="88" alt="">

# AgentPlus

统一管理编程 Agent 的 API 配置和模型列表。

[![Release](https://img.shields.io/github/v/release/cnklpz/AgentPlus?color=2F54EB)](https://github.com/cnklpz/AgentPlus/releases/latest)
[![Windows](https://img.shields.io/badge/Windows-10%20%7C%2011-0078D4?logo=windows&logoColor=white)](https://github.com/cnklpz/AgentPlus/releases/latest)
[![macOS](https://img.shields.io/badge/macOS-11%2B-000000?logo=apple&logoColor=white)](https://github.com/cnklpz/AgentPlus/releases/latest)
[![Tauri](https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white)](https://tauri.app)
[![License](https://img.shields.io/badge/license-AGPL--3.0-blue)](LICENSE)

[English](README.md) | 简体中文

</div>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/codex-dark-zh.png">
  <img src="docs/images/codex-zh.png" alt="AgentPlus 主界面">
</picture>

## 主要功能

- **供应商库**：API 地址和密钥填一次，所有 Agent 共用。内置 21 套模板，覆盖常见厂商和编程套餐，多数只要贴一个 Key。
- **模型列表**：拉取供应商提供的模型，再挑出每个 Agent 要显示的那些。
- **改动预览**：没看过 diff 就不会写入，被替换的文件先存到 `~/.agentplus/backups/`，之后随时能还原。
- **本地网关**：在 OpenAI Chat、OpenAI Responses、Anthropic Messages 之间互转，流式输出和工具调用都支持——Claude Code 要接一家只提供 OpenAI 接口的服务，靠的就是它。
- **连接测试**：先测延迟，再发一个真实请求，地址、密钥、模型能不能用，切换之前就知道。
- **WSL**：WSL 发行版里的 Agent 和 Windows 共用同一份供应商库。
- **隐私模式**：`Ctrl+Shift+H` 遮住密钥、服务地址和用户名，截图或共享屏幕之前按一下。

Agent 也能从应用里一键重启。另外还顺手做了：命令面板（`Ctrl+K`）、关掉窗口后让网关继续跑的托盘图标、安装前自校验签名的应用内更新，以及中英文界面。

## 安装

从 [Releases](https://github.com/cnklpz/AgentPlus/releases/latest) 下载最新版本。

**Windows 10/11（x64）**：`AgentPlus_<版本>_x64-setup.exe`。安装包还没签名，首次运行时 SmartScreen 可能会拦一下，点「更多信息 → 仍要运行」即可。

**macOS 11+（实验性支持）**：Apple 芯片选 `_aarch64.dmg`，Intel 芯片选 `_x64.dmg`。应用没有公证，需要在「系统设置 → 隐私与安全性」里放行。要是 macOS 一口咬定应用「已损坏」，执行：

```bash
xattr -cr /Applications/AgentPlus.app
```

暂不提供 Linux 安装包。

更新在启动时检查，可在「设置 → 通用 → 关于」中安装。

## 截图

<table>
  <tr>
    <td width="50%"><img src="docs/images/providers-zh.png" alt="供应商库"></td>
    <td width="50%"><img src="docs/images/codex-models-zh.png" alt="模型列表"></td>
  </tr>
  <tr>
    <td align="center">供应商库，按 API 服务分组</td>
    <td align="center">挑选 Codex 里显示哪些模型</td>
  </tr>
  <tr>
    <td colspan="2"><img src="docs/images/gateway-zh.png" alt="本地网关"></td>
  </tr>
  <tr>
    <td colspan="2" align="center">网关的流量、耗时和失败率</td>
  </tr>
</table>

## 支持的 Agent

AgentPlus 能识别 15 个 Agent，读取和写入这些配置文件：

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

列表里只会出现检测到的 Agent。装在非常规位置的话，可以在「设置 → Agent 识别」中指定目录。

<details>
<summary><b>Codex</b></summary>

- 固定供应商 ID，切换 API 服务后历史会话仍然可见。
- 模型列表按供应商保存，切换时自动加载。
- 浏览历史会话，查看某个会话为什么被隐藏。会话可以迁移到别的供应商（支持撤销），也可以复制 `codex resume` 命令。
- 检查数据库、缺失文件和过大的日志，清理前自动备份。
- 用 ChatGPT 登录后可以导入官方模型目录。
- 可选界面补丁：Fast 模式、显示完整模型名等。

</details>

<details>
<summary><b>Claude Code</b></summary>

- 每个供应商保存一套配置，切换时写进 `settings.json` 的 `env`。自己手写的配置也能导入进来。
- 设置默认模型，以及 Opus、Sonnet、Haiku 和子代理分别用哪个模型。
- 其他协议的 API 走本地网关，由它转换成 Claude Code 使用的 Anthropic 协议。
- 可以关掉非必要流量，以及 Git 提交里的 Co-Authored-By 署名。

</details>

<details>
<summary><b>OpenCode 和 Kilo Code</b></summary>

- 支持项目级配置，并标出哪些设置继承自全局配置。
- 常用设置用表单改：默认模型、`small_model`、启用的供应商、分享、自动更新、权限。
- 通过 `opencode auth` 登录的供应商只读显示。

</details>

<details>
<summary><b>ZCode 和 MiMo Desktop</b></summary>

- ZCode：模型顺序、各模型的上下文规则、思考过程显示、记忆、托盘。
- MiMo Desktop：账号内置的模型、技能目录、托盘行为、语音反馈。
- 两个应用都能从 AgentPlus 里直接重启。

</details>

<details>
<summary><b>其他 Agent</b></summary>

- Gemini CLI：多套配置之间切换。环境变量或项目里的 `.env` 覆盖了当前设置时，会给出提示。
- Kimi Code 和 Hermes：只重写有改动的配置块，注释和格式原样保留。
- CodeBuddy：IDE 和 CLI 共用一份配置，改完一秒内热加载。
- Droid：写入前重新读一遍配置，Droid 运行期间做的改动不会被覆盖。
- OpenClaw：地址或密钥变了，它为各 agent 生成的模型文件会一起更新。
- pi：按 pi 支持的格式写入，无效字段不会让整个文件读不出来。
- 配置里的 `$VAR`、`${VAR}`、`env_key` 等变量引用会原样保留。

</details>

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

想在浏览器里预览界面（用的是演示数据）可以跑 `npm run dev`。

## 许可证

[AGPL-3.0-only](LICENSE)。Copyright (C) 2026 cnklpz。

如需在 AGPL 条款之外使用（例如闭源分发），请联系作者获取商业授权。
