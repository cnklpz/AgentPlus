# 更新日志 / Changelog

每个版本一节，标题写 `## 版本号`。发布时这一节会成为 GitHub Release 的说明，也会显示在 AgentPlus 的更新提示里，所以中英文都写上。

Each version gets a `## <version>` section. On release it becomes the GitHub release text and is shown in AgentPlus's update prompt, so write it in both languages.

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
