# AgentPlus

简体中文 | [English](README.en.md)

一个桌面小工具，统一管理各家 AI 编程 Agent 的**供应商（API 地址 + 密钥）和模型列表**。

每个 Agent 的配置文件格式都不一样：Codex 是 TOML + `models.json`，OpenCode 是 JSONC，Claude Code 靠 `settings.json` 里的环境变量……
AgentPlus 把这些都读出来，放在同一个界面里编辑：供应商在「供应商」页维护一次，就能推送到任意 Agent，
再按 Agent 决定哪些模型出现在它的模型选择器里。

> AgentPlus 只管「有哪些供应商、选择器里有哪些模型」，**不替你选当前用哪个模型**，这个仍然在各 Agent 里自己选。

## 下载安装

到 [Releases](https://github.com/cnklpz/AgentPlus/releases/latest) 下载 `AgentPlus_<版本>_x64-setup.exe`，双击安装。

- 系统要求：Windows 10 / 11（x64）。WebView2 在 Windows 11 上自带，Windows 10 上安装程序会自动补装。
- 安装包未做代码签名，首次运行时 SmartScreen 可能提示「已保护你的电脑」，点「更多信息 → 仍要运行」即可。
- **macOS / Linux 暂不支持**：代码能编译，但 Agent 识别、重启、打开文件夹等功能目前只实现了 Windows 版本。

### 更新

AgentPlus 启动后会到 GitHub Releases 检查新版本，有新版本时右上角的「设置」按钮会出现一个小圆点。
到「设置 → 通用 → 关于」可以查看更新说明，点「下载并安装」后自动下载、校验签名、安装并重新打开。

不想自动检查的，关掉同一处的「启动时检查更新」，之后需要时手动点「检查更新」。

## 支持的 Agent

| Agent | 管理的配置 |
|---|---|
| Codex（桌面版 + CLI） | `~/.codex/config.toml`、模型目录 `models.json`，密钥在 `~/.codex/.env` |
| Claude Code | 供应商作为配置档，切换时写入 `~/.claude/settings.json` 的 `env` |
| OpenCode | `~/.config/opencode/opencode.json(c)`，密钥在 `auth.json`；支持项目级配置 |
| MiMo Desktop | `~/.config/mimocode/mimocode.jsonc` |
| ZCode | `~/.zcode/v2/provider_config.json` |
| Gemini CLI | `~/.gemini/.env` + `settings.json` |
| Qwen Code | `~/.qwen/settings.json` |
| Kimi Code | `~/.kimi-code/config.toml` |
| Kilo Code | `~/.config/kilo/kilo.json(c)` |
| CodeBuddy | `~/.codebuddy/models.json` |
| Droid (Factory) | `~/.factory/settings.json` |
| Hermes | HERMES_HOME 下的 `config.yaml` |
| pi | `~/.pi/agent/models.json` |
| OpenClaw | `~/.openclaw/openclaw.json` |
| Trae | 仅识别（自定义模型存在账号云端，无法代写），给出手动操作步骤 |

未检测到安装的 Agent 会自动隐藏；装在非默认位置的，可以在「设置 → Agent 识别」里手动指定配置目录，
也可以在那里把识别到但用不上的 Agent 从侧边栏隐藏。

## 主要功能

- **供应商库**：所有供应商集中在一处，增删改后推送到选中的 Agent。内置常见厂商和编程套餐的模板，只需填 API Key；
  可以直接从供应商拉取模型列表，测延迟，或发一个真实的小请求测试连通性。
- **模型列表**：按 Agent 管理选择器里显示哪些模型，支持上下文窗口等字段。
- **先预览、后应用**：所有改动先进入待应用列表，可逐条查看 diff 再写入；写完可以一键重启对应的 Agent 让配置生效。
- **历史与回滚**：每次写入前把原文件备份到 `~/.agentplus/backups/`，可一键回滚。
- **本地网关**：`127.0.0.1` 上的小型 HTTP 服务，在 OpenAI Chat Completions、OpenAI Responses、Anthropic Messages 三种协议之间实时转换（含流式）。
  让只支持 Responses 的 Codex、只支持 Anthropic 协议的 Claude Code 也能用其他协议的中转；带出错熔断，每个 Agent 使用独立的网关密钥。
- **WSL**：目标环境可在 Windows 和 WSL 发行版之间切换，供应商库在各环境间共用。
- **Codex 专项**：会话浏览、健康检查、安全清理和供应商修复；从官方拉取模型列表；Fast 模式与完整模型名的界面补丁。
- **OpenCode 项目配置**：给单个项目文件夹单独配置供应商、默认模型和权限，和全局配置按 OpenCode 的规则合并。
- **托盘与隐私模式**：关闭窗口可以最小化到托盘，网关继续运行；隐私模式（`Ctrl+Shift+H`）遮挡密钥、地址和用户名，方便截图和共享屏幕。
- **命令面板**：`Ctrl+K` 搜索供应商、模型、设置和会话。
- **应用内更新**：见上文「更新」。
- **多设备同步**（暂未开放）：通过共享文件夹导出/导入供应商和模型列表，不导出 API Key。

界面支持简体中文和英文，在「设置 → 界面 → 语言」切换，默认跟随系统。

## 开发

技术栈：[Tauri 2](https://tauri.app)（Rust，`src-tauri/`）+ React 18 + TypeScript（`src/`）+ Vite。

准备环境：Node.js 18+、Rust stable（1.88 及以上），以及 [Tauri 的系统依赖](https://tauri.app/start/prerequisites/)（Windows 上是 WebView2 和 MSVC 生成工具）。

```bash
npm install
npm run tauri dev      # 开发模式运行
npm run tauri build    # 打包安装程序（本地打包不需要签名密钥，见下文）
```

只改界面时可以用 `npm run dev` 在浏览器里预览，后端调用会换成本地的演示数据。

检查与测试：

```bash
npm run check                                   # 前端：类型检查 + 单元测试（vitest）
cd src-tauri && cargo clippy --all-targets && cargo test   # 后端：lint + 单元测试
```

`cargo test` 里标了 `#[ignore]` 的用例会读本机真实的 Agent 配置（只读），需要时用
`cargo test <名字> -- --ignored --nocapture` 单独运行。

### 目录结构

```
src/                    前端
  components/           页面与组件
  i18n/zh, i18n/en      界面文案（中文为源语言）
  api.ts                调用后端命令
  updater.ts            应用内更新的状态
src-tauri/src/          后端
  adapters/             每个 Agent 一个适配器，负责读写它的配置文件
  gateway/              本地网关：协议转换、HTTP 服务、熔断、密钥
  sessions.rs           Codex 会话管理
  history.rs            备份与回滚
  update.rs             应用内更新（tauri-plugin-updater）
  i18n.rs               后端文案双语
scripts/                发版用的小脚本
docs/design.html        设计方案
```

AgentPlus 自己的数据保存在 `~/.agentplus/`（`store.json` 和 `backups/`）。

多语言和代码约定见 [CLAUDE.md](CLAUDE.md)：界面上的文字不许写死，新增文案要同时写中英文。

## 发布新版本

发版由 GitHub Actions 完成（[`.github/workflows/release.yml`](.github/workflows/release.yml)）：推送 `v*` 标签后，
在 Windows 上打包、用私钥给安装包签名，并创建一个**草稿** Release，附带安装包、签名和给自动更新用的 `latest.json`。

**一次性准备**

1. 仓库需要是公开的（私有仓库的 Release 未登录访问不到，自动更新会失败）。
2. 签名密钥：公钥已写在 `src-tauri/tauri.conf.json` 的 `plugins.updater.pubkey`；
   私钥**只保存在维护者本地**，把它的内容添加为仓库的 Actions secret `TAURI_SIGNING_PRIVATE_KEY`
   （Settings → Secrets and variables → Actions）。私钥设了密码的，再加一个 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`。

   > 私钥丢了，已安装的旧版本就再也收不到更新（只能让用户手动下载新安装包），务必备份。

**每次发版**

1. 在 [CHANGELOG.md](CHANGELOG.md) 顶部加一节 `## 0.2.0`，写上中英文更新说明，提交。
2. 升版本号并打标签（工作区要干净）：

   ```bash
   npm version 0.2.0      # 同步 package.json、tauri.conf.json、Cargo.toml、Cargo.lock，提交并打 v0.2.0 标签
   git push --follow-tags
   ```

3. 等 Actions 里的 Release 工作流跑完（约 10 分钟），到 Releases 页面检查草稿，确认无误后点 **Publish release**。
   发布之后，已安装的 AgentPlus 在下次启动或手动检查时就会看到这个版本。

发错了版本：把该 Release 改回草稿或删除即可，客户端只认最新的正式 Release。

## 参与贡献

欢迎通过 [Issues](https://github.com/cnklpz/AgentPlus/issues) 反馈问题和建议。

目前暂不接受代码贡献（Pull Request）。以后开放时，提交代码前需要签署贡献者许可协议（CLA），
授权作者以 AGPLv3 以外的方式（包括商业授权）使用你贡献的代码。

## 许可证

Copyright (C) 2026 cnklpz

本项目以 [GNU Affero General Public License v3.0](LICENSE)（SPDX：`AGPL-3.0-only`）发布。
修改并分发本项目，或通过网络向他人提供修改后的版本时，需要按 AGPLv3 向对方提供对应的完整源代码。

如需在不满足 AGPLv3 条款的情况下使用（例如闭源分发），请联系作者获取商业授权。
