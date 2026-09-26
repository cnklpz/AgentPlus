# 更新日志 / Changelog

每个版本一节，标题写 `## 版本号`。发布时这一节会成为 GitHub Release 的说明，也会显示在 AgentPlus 的更新提示里，所以中英文都写上。

Each version gets a `## <version>` section. On release it becomes the GitHub release text and is shown in AgentPlus's update prompt, so write it in both languages.

## 0.1.3

模型参数智能匹配：添加模型时按模型 ID 自动填上上下文窗口、最大输出、能读的输入类型（图片、PDF 等）、是否推理和工具调用，和 ZCode 的做法类似。OpenCode Go / Zen 模板支持多协议。

- 资料来自内置表（models.dev 上厂商官方的条目）和 models.dev 完整目录（后台每周更新一次）；带厂商前缀、日期后缀的 ID 也能认出来
- 添加模型时边输入边填，标着「自动匹配」，可以再改；编辑已有模型可以点「智能匹配」，只补还空着的项
- 拉取模型后批量添加、编辑供应商时新增的模型、新建供应商或从供应商页推送过去的模型，都会自动带上
- 按各 Agent 自己的字段写入（OpenCode、Kilo、MiMo、pi、OpenClaw、CodeBuddy、Droid、Codex、Qwen Code、Kimi Code）；ZCode 自己会匹配，不插手
- Codex 新增的自定义模型不再照抄别的模型的上下文和「能读图片」
- OpenCode Go / Zen 模板可以多选接口类型，每个模型走它自己的协议；经本地网关时合成一个入口，同一模型优先走不用转换的协议
- OpenCode Go 要求请求带会话 ID：本地网关转发和连通性测试会自动补上
- 本地网关能统计更多供应商的用量（如 Moonshot）
- Agent 图标换成各自的官方标识

macOS（实验性）：修复 Codex、Claude 等 Agent 识别不到的问题，窗口操作改成 Mac 的习惯。

- 按 Bundle ID 识别 App：Codex 现在是 ChatGPT.app（旧的 Codex.app 也认），Claude Desktop（Claude.app）算作已装 Claude Code
- 更新 ZCode、CodeBuddy、Trae、OpenCode、OpenClaw、MiMo Desktop 的 Mac 识别方式
- 补上 pi、Kilo、Hermes、CodeBuddy 命令行版的安装位置；读不到登录 shell 的 PATH 时也会搜 ~/.local/bin、Homebrew 等常见位置
- Mac 上只要默认位置有配置文件，就会显示对应的 Agent
- 窗口：红绿灯按钮在顶栏居中，顶栏不再显示图标和名字；关闭窗口只是隐藏，AgentPlus 留在 Dock 里继续运行（本地网关不中断），点 Dock 图标回来，⌘Q 退出；不再使用菜单栏托盘图标
- 程序名显示为 AgentPlus（原来是小写的 agentplus）

Smart model settings: when you add a model, its context window, max output, input kinds (images, PDFs…), reasoning and tool calls are filled in from its ID, much like ZCode does. The OpenCode Go / Zen templates support several protocols.

- The data comes from a built-in table (the vendors' own entries on models.dev) and models.dev's full catalog (refreshed weekly in the background); IDs with a vendor prefix or a date suffix are recognized too
- In the add-model dialog the settings fill in as you type, marked "Auto-filled", and you can still change them; for an existing model, "Smart match" fills in only what isn't set yet
- Models added in bulk after fetching, models added while editing a provider, and the models of a new provider (or one pushed from the Providers page) get their settings too
- Written in each agent's own fields (OpenCode, Kilo, MiMo, pi, OpenClaw, CodeBuddy, Droid, Codex, Qwen Code, Kimi Code); ZCode matches models itself and is left alone
- New custom Codex models no longer copy another model's context window and image support
- The OpenCode Go / Zen templates let you pick several API types, each model on its own protocol; through the local gateway they share one entry, and a model goes to the protocol that needs no conversion first
- OpenCode Go requires a session ID on every request: the local gateway and the connection test add it
- The local gateway counts token usage for more providers (e.g. Moonshot)
- Agent icons are now each agent's official mark

macOS (experimental): fixes Codex, Claude and other agents not being detected, and makes the window behave the Mac way.

- Apps are matched by bundle ID: Codex is now ChatGPT.app (an older Codex.app still counts), and Claude Desktop (Claude.app) counts as Claude Code
- Updated macOS detection for ZCode, CodeBuddy, Trae, OpenCode, OpenClaw and MiMo Desktop
- Added the install locations of pi, Kilo, Hermes and the CodeBuddy CLI; when the login shell's PATH can't be read, ~/.local/bin, Homebrew and other common folders are still searched
- On macOS, an agent shows up whenever its config exists in the default location
- Window: the traffic lights are centered in the top bar, which no longer shows the icon and name. Closing the window only hides it: AgentPlus stays in the Dock and keeps running (the local gateway included); click the Dock icon to bring it back, press ⌘Q to quit. No menu bar tray icon any more
- The app's process is now named AgentPlus (was lowercase agentplus)

## 0.1.1

新增 macOS 版（实验性，Apple 芯片和 Intel 通用）。

- 识别 /Applications 里的桌面版 Agent 和 PATH 上的命令行版，可重启桌面版
- 使用系统原生标题栏按钮，快捷键显示为 ⌘，在访达中打开文件夹
- 应用没有经过 Apple 公证：首次打开时到「系统设置 → 隐私与安全性」点「仍要打开」；如果提示「已损坏」，在终端运行 `xattr -cr /Applications/AgentPlus.app`
- 还没在真机上充分测试，遇到问题欢迎反馈

Adds a macOS build (experimental, universal for Apple silicon and Intel).

- Finds desktop agents in /Applications and CLIs on PATH, and restarts desktop apps
- Native title bar buttons, ⌘ shortcuts, folders open in Finder
- The app isn't notarized by Apple: on first launch, click "Open Anyway" in System Settings → Privacy & Security. If it says the app is damaged, run `xattr -cr /Applications/AgentPlus.app` in Terminal
- Not yet tested much on real Macs; feedback welcome

## 0.1.0

首个公开版本。

- 供应商库：供应商集中维护，推送到 Codex、Claude Code、OpenCode、ZCode、MiMo Desktop 等 15 个 Agent
- 按 Agent 管理模型选择器里的模型；改动先预览 diff 再写入，写入前自动备份，可一键回滚
- 本地网关：OpenAI Chat / Responses / Anthropic Messages 协议互转，带出错熔断
- Codex 会话管理、健康检查与清理；OpenCode 项目级配置；WSL 环境切换
- 托盘、隐私模式、中英文界面
- 应用内更新：从 GitHub Releases 检查新版本，下载后校验签名再安装

First public release.

- Provider library: keep providers in one place and push them to 15 agents, including Codex, Claude Code, OpenCode, ZCode and MiMo Desktop
- Per-agent model lists; every change is previewed as a diff, backed up before writing and can be rolled back
- Local gateway converting between OpenAI Chat, OpenAI Responses and Anthropic Messages, with an error breaker
- Codex session browser, health check and cleanup; OpenCode per-project config; WSL targets
- Tray icon, privacy mode, Chinese and English UI
- In-app updates: checks GitHub Releases, verifies the installer's signature before installing
