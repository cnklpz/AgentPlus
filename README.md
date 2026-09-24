# AgentPlus

一个桌面小工具，统一管理各家 AI 编程 Agent 的**供应商（API 地址 + 密钥）和模型列表**。

每个 Agent 的配置文件格式都不一样：Codex 是 TOML + `models.json`，OpenCode 是 JSONC，Claude Code 靠 `settings.json` 里的环境变量……
AgentPlus 把这些都读出来，放在同一个界面里编辑：供应商在「供应商」页维护一次，就能推送到任意 Agent，
再按 Agent 决定哪些模型出现在它的模型选择器里。

> AgentPlus 只管「有哪些供应商、选择器里有哪些模型」，**不替你选当前用哪个模型**，这个仍然在各 Agent 里自己选。

界面支持简体中文和英文。

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

未检测到安装的 Agent 会自动隐藏；装在非默认位置的，可以在「设置 → Agent 识别」里手动指定配置目录。

## 主要功能

- **供应商库**：所有供应商集中在一处，增删改后推送到选中的 Agent。内置常见厂商和编程套餐的模板，只需填 API Key。
- **模型列表**：按 Agent 管理选择器里显示哪些模型，支持上下文窗口等字段。
- **先预览、后应用**：所有改动先进入待应用列表，可逐条查看 diff 再写入。
- **历史与回滚**：每次写入前把原文件备份到 `~/.agentplus/backups/`，可一键回滚。
- **本地网关**：`127.0.0.1` 上的小型 HTTP 服务，在 OpenAI Chat Completions、OpenAI Responses、Anthropic Messages 三种协议之间实时转换（含流式）。
  让只支持 Responses 的 Codex、只支持 Anthropic 协议的 Claude Code 也能用其他协议的中转；带出错熔断，每个 Agent 使用独立的网关密钥。
- **WSL**：目标环境可在 Windows 和 WSL 发行版之间切换。
- **多设备同步**：通过共享文件夹（网盘、网络共享、U 盘）导出/导入供应商和模型列表。**不会导出 API Key**，导入后同样先进入待应用列表。
- **Codex 专项**：会话浏览、健康检查、安全清理和供应商修复；从官方拉取模型列表；Fast 模式与完整模型名的界面补丁。
- **OpenCode 项目配置**：给单个项目文件夹单独配置供应商、默认模型和权限，和全局配置按 OpenCode 的规则合并。

## 开发

技术栈：[Tauri 2](https://tauri.app)（Rust，`src-tauri/`）+ React 18 + TypeScript（`src/`）+ Vite。

准备环境：Node.js 18+、Rust stable，以及 [Tauri 的系统依赖](https://tauri.app/start/prerequisites/)（Windows 上是 WebView2 和 MSVC 生成工具）。

```bash
npm install
npm run tauri dev      # 开发模式运行
npm run tauri build    # 打包安装程序
```

检查与测试：

```bash
npx tsc --noEmit                          # 前端类型检查
cd src-tauri && cargo check && cargo test # 后端
```

### 目录结构

```
src/                    前端
  components/           页面与组件
  i18n/zh, i18n/en      界面文案（中文为源语言）
  api.ts                调用后端命令
src-tauri/src/          后端
  adapters/             每个 Agent 一个适配器，负责读写它的配置文件
  gateway/              本地网关：协议转换、HTTP 服务、熔断、密钥
  sessions.rs           Codex 会话管理
  history.rs            备份与回滚
  sync.rs               多设备同步
  i18n.rs               后端文案双语
docs/design.html        设计方案
```

AgentPlus 自己的数据保存在 `~/.agentplus/`（`store.json` 和 `backups/`）。

多语言和代码约定见 [CLAUDE.md](CLAUDE.md)：界面上的文字不许写死，新增文案要同时写中英文。

## 许可证

[MIT](LICENSE)
