# 支持的 Agent

[English](agents.md) | 简体中文

AgentPlus 能识别 15 个 Agent。下面是它读取和写入的配置文件，以及每个 Agent 分别支持哪些操作。汇总表格见 [README](../README.zh-CN.md#支持的-agent)。

只有检测到的 Agent 才会出现在应用里。装在非常规位置的话，可以在「设置 → Agent 识别」中指定目录。此外，写入配置时 `$VAR`、`${VAR}`、`env_key` 这类变量引用一律原样保留，不会被展开。

## Codex

`~/.codex/config.toml` 和 `models.json`。桌面版和 CLI。

- 固定供应商 ID，切换 API 服务后历史会话仍然可见。
- 模型列表按供应商分别保存，切换时自动加载。
- 浏览历史会话，查看某个会话为什么被隐藏。会话可以迁移到别的供应商（支持撤销），也可以复制 `codex resume` 命令。
- 检查数据库、缺失文件和过大的日志，清理前自动备份。
- 用 ChatGPT 登录后可以导入官方模型目录。
- 可选界面补丁：Fast 模式、显示完整模型名等。

## Claude Code

`~/.claude/settings.json` 里的 `env`。

- 每个供应商一套配置，切换时写进 `env`。自己手写的配置也能导入进来。
- 设置默认模型，以及 Opus、Sonnet、Haiku 和子代理分别用哪个模型。
- 其他协议的 API 走本地网关，由它转换成 Claude Code 使用的 Anthropic 协议。
- 可以关掉非必要流量，以及 Git 提交里的 Co-Authored-By 署名。

## OpenCode 和 Kilo Code

`~/.config/opencode/opencode.json(c)` 和项目目录下的 `opencode.json`；`~/.config/kilo/kilo.json(c)`。

- 支持项目级配置，从全局配置继承来的设置会标出来。
- 常用设置用表单改：默认模型、`small_model`、启用的供应商、分享、自动更新和权限。
- 通过 `opencode auth` 登录的供应商只读显示。

## ZCode 和 MiMo Desktop

`~/.zcode/v2/provider_config.json`；`~/.config/mimocode/mimocode.jsonc`。

- ZCode：模型顺序、各模型的上下文规则，以及思考过程显示、记忆、托盘等选项。
- MiMo Desktop：账号内置模型、技能目录、托盘行为和语音反馈。
- 两个应用都可以直接从 AgentPlus 重启。

## Gemini CLI

`~/.gemini/settings.json`。

- 多套配置之间切换。环境变量或项目里的 `.env` 覆盖当前设置时，会给出提示。

## Qwen Code

`~/.qwen/settings.json`。

- 按协议和地址把 `modelProviders` 里的模型归组显示。密钥以 `envKey` 引用的形式保留，实际值写进 `env` 块；改完 Qwen Code 会热加载。

## Kimi Code 和 Hermes

`~/.kimi-code/config.toml`；`$HERMES_HOME/config.yaml`。

- 只重写有改动的配置块，注释和格式原样保留。

## CodeBuddy

`~/.codebuddy/models.json`。

- IDE 和 CLI 共用一份配置，改完一秒内热加载。

## Droid（Factory）

`~/.factory/settings.json`。

- 写入前重新读一遍配置，Droid 运行期间的改动不会被覆盖。

## OpenClaw

`~/.openclaw/openclaw.json`。

- 地址或密钥变了，它为各 agent 生成的模型文件会一起更新。

## pi

`~/.pi/agent/models.json`。

- 按支持的格式写入配置，不会因为无效字段导致整个文件读不出来。

## Trae

仅支持识别。

- 自定义模型保存在账号里，需要按 AgentPlus 给出的步骤手动添加。
