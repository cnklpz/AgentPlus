# Changelog / 更新日志

Each version gets one section headed `## <version>`. On release, the text of that section becomes the GitHub release notes and is shown in AgentPlus's in-app update prompt, so it is written in both English and Chinese.

## 0.2.2

「过度」动画下滚动到边界会被拉扯变形，连续硬拉会把页面扯碎；触控板在边界不再一顿一顿。

- 「过度」动画：滚动到顶部或底部后继续滑，内容像橡皮一样被拉长变形，松手后回弹时会轻微压扁再复原
- 「过度」动画：在同一边界连续拉扯多次后还继续拉，页面会被扯碎，各块内容四散飞开；往反方向滑动可以一点点拼回来，按 Esc 立即复原
- 触控板在滚动边界不再一顿一顿：手指减速时不再被误当成松手而突然弹回
- 设置里「检查更新」的按钮和左侧文字垂直居中

At the Excessive motion level, scrolling past an edge stretches the page, and pulling hard again and again tears it apart; the trackpad no longer stutters at edges.

- Excessive motion: scrolling past the top or bottom stretches the content like rubber, and it springs back with a slight squash
- Excessive motion: keep pulling the same edge several times in a row and the page tears, its blocks flying apart; scroll the other way to gather them back bit by bit, or press Esc to restore it at once
- The trackpad no longer stutters at a scroll edge: fingers slowing down are no longer mistaken for a lift that snapped the stretch back
- The Check for updates buttons in Settings are centred beside their text

## 0.2.1

官方供应商也能测速了。

- 官方内置供应商（如 Codex 的 ChatGPT 账号登录、Claude 官方账号、Gemini CLI 的 Google 登录、OpenCode 的内置供应商）现在也会测速，测的是它们实际连接的官方接口，只计往返时间，不发送账号信息
- 「过度」动画下，开启明文同步 API Key 的全屏警告关闭时也有退出动画

Built-in official providers can be tested for latency too.

- Built-in official providers (such as Codex's ChatGPT sign-in, the Claude official account, Gemini CLI's Google sign-in and OpenCode's built-in providers) now show a latency, measured against the official endpoint they connect to; only the round trip is timed and no account data is sent
- At the Excessive motion level, the full-screen warning for syncing API keys in plain text now plays an exit animation when it closes

## 0.2.0

多设备同步上线：可加密、可自动同步、保留同步记录；macOS 有了菜单栏。

- 多设备同步（侧边栏「多设备同步」）：选一个网盘、NAS 或 U 盘里的文件夹，在设备间同步供应商库和各 Agent 的供应商、模型列表
- 同步密码：可以自己输入，也可以生成随机密钥（能复制或保存为 txt）。设置后同步文件用 AES-256-GCM 加密（密钥由 Argon2id 派生），密码保存在本机，由 Windows 加密保护
- 「同步 API Key」：加密时可以连同 API Key 一起同步，导入的供应商直接可用。没设密码也能开，但会以明文写入，开启前会醒目警告
- 自动同步：可选打开软件时同步、有变更时同步。另一台设备先同步了新内容时，会先提醒你对比导入，不会覆盖
- 同步记录：每次导出都保留一份（最多 5–50 份可选），点一条可以和本机对比并恢复，也可以删除
- macOS：新增菜单栏，有设置（⌘,）、检查更新、各页面和标准的编辑菜单
- 设置说明随选项变化时，新旧文字淡入淡出切换
- 切换动画级别时，页面标题等不再重播入场动画
- 触控板惯性滚动到边缘时，回弹不再卡住，会立即弹回

Multi-device sync is here, with encryption, automatic sync and sync records; macOS gets a menu bar.

- Multi-device sync (Multi-device sync in the sidebar): pick a folder on a cloud drive, NAS or USB stick to sync the provider library and every agent's providers and model lists between devices
- Sync password: type one or generate a random key (copy it or save it as .txt). The sync file is then encrypted with AES-256-GCM (key derived with Argon2id), and the password is kept on this device, protected by Windows
- "Sync API keys": encrypted files can carry the API keys, so imported providers work right away. It also works without a password, but then the keys are written in plain text, after a prominent warning
- Automatic sync: optionally sync when AgentPlus starts and after changes. When another device synced something new first, you're asked to compare and import it instead of it being overwritten
- Sync records: every export keeps a copy (keep 5–50); click one to compare it with this device and restore from it, or delete it
- macOS: a menu bar with Settings (⌘,), Check for Updates, the pages and the standard Edit menu
- Setting descriptions fade between old and new wording when their value changes
- Switching the motion level no longer replays entrance animations such as the page title
- Trackpad momentum no longer holds the overscroll bounce at the edge; it springs back right away

## 0.1.8

提示文字切换有了动画；macOS 改用苹方和系统字体。

- 在设置 → 界面里切换「详细」「简约」提示文字时带动画：「过度」动画下提示从末尾逐字碎裂、逐字显现，「标准」下整段淡入淡出并收起展开，「减弱」下只淡入淡出
- macOS：界面改用系统字体（英文 SF，中文苹方）。以前装了 Office 的 Mac 会用微软雅黑显示界面
- 「Extra」动画级别在英文界面里改名为「Excessive」

Switching hint text now animates; macOS uses PingFang and the system font.

- Switching between Detailed and Brief hint text (Settings → Interface) animates: at Excessive motion the hints shatter and come back character by character, at Standard whole hints fade and fold, at Reduced they only fade
- macOS: the interface uses the system font (SF, with PingFang for Chinese). Macs with Office installed used to show it in Microsoft YaHei
- The "Extra" motion level is renamed "Excessive" in the English interface

## 0.1.7

网关不再接受旧的共用密钥；回滚 Claude Code、Gemini CLI 配置后模型不再变回去；Gemini 供应商可以获取模型列表和测试。

- 网关不再接受旧的共用密钥 `agentplus-gateway`。升级后如果网关页提示有 Agent 还在用旧密钥，点「更新密钥」，否则这些 Agent 连不上网关
- 回滚 Claude Code、Gemini CLI 的配置时，AgentPlus 里的配置档一起恢复，之后再改设置不会又把新模型写回去。这两个 Agent 升级前的旧备份只能手动恢复
- 只改配置档（比如隐藏某个模型）不再生成备份，历史页清爽些
- Gemini 协议的供应商改用 Gemini 原生接口获取模型列表和测试连接（支持分页，被拦截或没有文本的回复会报错）
- 在网关页添加的供应商获取模型列表时，网关内部密钥只留在后端
- macOS：AgentPlus 写入的配置和密钥文件只有自己能读（新文件 0600、数据目录 0700），已有配置保留原来的权限
- 写配置时中断留下的临时文件会自动清理
- 诊断日志：路径里和你用户名开头相同的其他用户名，不再被错当成你的主目录
- Windows 安装程序支持简体中文

The gateway no longer accepts the old shared key; rolling back Claude Code or Gemini CLI configs no longer drifts back to the newer model; Gemini providers can list models and be tested.

- The gateway no longer accepts the old shared key `agentplus-gateway`. If the gateway page says an agent still uses the old key after the update, click "Update keys", or that agent can't connect
- Rolling back a Claude Code or Gemini CLI config also restores its AgentPlus profiles, so the next change no longer writes the newer model back. Backups of these two agents made before this version can only be restored manually
- Editing only profiles (e.g. hiding a model) no longer creates a backup, keeping the history page tidy
- Providers using the Gemini protocol list models and test connections through the native Gemini API (with pagination; blocked or empty replies are reported)
- When you add a provider on the gateway page, the gateway's internal key stays in the backend while models are listed
- macOS: config and key files AgentPlus writes are readable only by you (new files 0600, data folder 0700); existing configs keep their permissions
- Temp files left by an interrupted config write are cleaned up
- Diagnostic log: another user name that merely starts with yours is no longer mistaken for your home folder
- The Windows installer is available in Simplified Chinese

## 0.1.6

适配 Codex 26.924；重启 Agent 更快；更新完成后会提示。

- 适配 Codex 26.924：「隐藏用量提示横幅」重新生效
- 重启 Agent 时结束进程更快（先结束主进程，避免子进程被重新拉起），所有桌面端 Agent 都受益
- Codex 界面注入更快：跳过不含界面脚本的窗口，悬浮窗不再白等，每次重启少等十几秒
- 应用内更新完成、重新打开后，底部提示「已更新到 AgentPlus x.y.z」

Supports Codex 26.924; restarting agents is faster; AgentPlus tells you when an update is done.

- Codex 26.924: "Hide usage banners" works again
- Restarting an agent ends its processes faster (the main process goes first, so child processes aren't relaunched); applies to every desktop agent
- Codex UI patching is faster: windows without UI scripts are skipped and the overlay no longer waits for nothing, saving 10+ seconds per restart
- After an in-app update, AgentPlus shows "Updated to AgentPlus x.y.z" when it reopens

## 0.1.5

修复 Codex 自动更新后 AgentPlus 显示它没在运行的问题；新增诊断日志、启动方式提示和简约提示。

- Codex 自动更新后，AgentPlus 不用重启也能认出新版本和它的运行状态
- 诊断日志（设置 → 通用）：记录报错、网关失败的请求、重启和识别过程，出问题时一键导出分析；只存在本机，写入前去掉密钥、令牌、中转地址、IP、邮箱和路径里的用户名；按保留天数自动清理，最多 50 MB
- 右侧「当前配置」显示 Agent 是否由 AgentPlus 启动；Codex 的界面增强（Fast、完整模型名等）没生效时，会提示在 AgentPlus 里重启 Codex
- 设置 → 界面 → 提示文字：选「简约」后只显示选项名，隐藏说明文字
- 打开 AgentPlus 时各 Agent 同时检测，列表出来得更快
- macOS（实验性）：改进启动 Codex 后要等十几秒才识别到的问题（重启期间不让 AgentPlus 被系统节能挂起；窗口重新显示时立刻检查）。如果还慢，请导出诊断日志反馈

Fixes AgentPlus showing Codex as not running after Codex updated itself; adds a diagnostic log, a launch indicator and brief hints.

- After Codex updates itself, AgentPlus picks up the new version and its running state without a restart
- Diagnostic log (Settings → General): records errors, failed gateway requests, restarts and detection, and exports in one click for analysis. It stays on this computer; keys, tokens, relay addresses, IPs, emails and the user name in paths are removed before writing. Old logs are deleted after the days you choose, 50 MB at most
- The "Current config" panel shows whether the agent was launched by AgentPlus; when Codex's UI enhancements (Fast, full model names…) aren't active, it tells you to restart Codex from AgentPlus
- Settings → Interface → Hint text: "Brief" shows only option names, without descriptions
- Agents are detected in parallel when AgentPlus opens, so the list appears sooner
- macOS (experimental): works on Codex taking 10+ seconds to be detected after it starts (AgentPlus no longer gets napped by the system during a restart, and checks again as soon as its window shows). If it's still slow, please send an exported diagnostic log

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
