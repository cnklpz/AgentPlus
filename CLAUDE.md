# AgentPlus 项目规范

Tauri 2（Rust，`src-tauri/`）+ React 18 + TypeScript（`src/`）。
检查：前端 `npm run check`（`tsc --noEmit` + `vitest run`），后端在 `src-tauri/` 下
`cargo clippy --all-targets`（保持零警告）/ `cargo test`。改动逻辑时给边界情况补单元测试。

## 文案风格（界面、README、文档）

写给人看，不要写成模板。README 和文档最容易写成模板腔。

- 别让一屏里的条目都是同一个句式（八条「**标签.** 一句话」最典型）。长短错开，个别条目可以不带标签。
- 删掉只复述结构的句子（「下表说明了各 Agent 的支持范围」「此外还支持…」），直接写内容。
- 用具体事实和数字，不用概括性形容（「安装包还没签名」，不是「安装包暂未经过完善的安全签名认证」）。
- 不写无实义的三段并列（「快速、简单、可靠」），不堆「无缝」「一站式」「助力」「旨在」，英文同理避开
  seamless / effortless / powerful / comprehensive。
- 中英文各按母语习惯写，不逐字对译，两边句子数量不必相同。
- 「优化文案」指的是改文字，不是砍内容：不要顺手删条目、合并章节或把细节搬到别的文件。真想精简先单独确认。
- 文档里的数字（模板数、Agent 数、快捷键、路径）改了代码要一起核对：模板数看 `src/templates.ts` 的
  `TEMPLATES`，Agent 列表看 `src-tauri/src/adapters/`。

## 多语言（i18n）

界面支持简体中文和英文。**中文是源语言**，英文必须与之逐条对应。
**任何会显示给用户的文字都不许直接写死在代码里**，前后端都一样：新增或修改文案时，
同时写好中文和英文。

### 前端（`src/i18n/`）

- 词典：`src/i18n/zh/<ns>.ts` 是源，`src/i18n/en/<ns>.ts` 用 `const en: typeof zh` 做类型约束，
  少 key、多 key 都会编译报错。一个源文件一个命名空间（文件头注释写了对应的源文件），
  如 `components/GatewayPage.tsx` → `gatewayPage`。多个文件都要用的词放 `common`，别的一律放自己的命名空间。
- 新增源文件时：在 `zh/`、`en/` 各建一个同名命名空间文件，并在两边的 `index.ts` 里注册。
- 用法（`import { t, tn, tx } from "../i18n"`）：
  - `t("gatewayPage.title")`，占位符 `t("app.saved", { name })`，词典里写 `"已保存「{name}」"`。
  - 数量相关用 `tn("x.models", n)`，英文写 `"{n} model|{n} models"`（单数|复数），中文只写一种。
  - 占位符要放 React 元素时用 `tx("x.hint", { link: <a …/> })`，不要把一句话拆成好几个 key 拼接。
  - key 用 camelCase、按含义命名（`deleteConfirm`），不要用 `text1`；句子整句成一个 key，别按词拼接，英文语序和中文不同。
- 语言切换时 `App` 会重新渲染并重新拉取后端数据。所以：
  - **不要在模块顶层调用 `t()`**（`const TABS = [{ label: t(...) }]` 只会求值一次）。顶层常量里存 key（类型 `TKey`）或写成函数，渲染时再 `t()`。
  - `useMemo` 里产生译文的，依赖里加上 `useLang()` 的返回值。
  - 组件自己在 mount 时从后端拉数据并存在 state 里的（后端文案也会随语言变），effect 依赖里加 `lang`。
- 日期、数字格式化用 `toLocaleString(locale())`，不要写死 `"zh-CN"`。
- 不翻译：Agent / 产品名（Codex、Claude Code、OpenCode…）、配置键名、文件路径、协议名、命令行。
- 英文风格：简洁、句首大写（sentence case），按钮用动词（Save、Add provider）；中文里的「」换成英文引号 "…"，全角标点换成半角。
- 语言偏好在 `prefs.lang`（`auto | zh | en`，auto 跟随系统），设置页「界面」里切换。

### 后端（`src-tauri/src/i18n.rs`）

后端生成的文字（`Kv` 标签、notes、diff 行、错误信息、设置项的 label/desc 等）同样要双语：

- 没有占位符：`l("地址", "Base URL")` → `&'static str`。
- 有占位符：`tr!("找不到供应商 {id}", "Provider not found: {id}")` → `String`，规则同 `format!`（字面的 `{` `}` 要写成 `{{` `}}`）。
  错误：`anyhow!(tr!(…))` / `bail!("{}", tr!(…))`。
- 静态表（`const` 数组、`Spec` 等）不能调用 `l()`：表里存 `(zh, en)` 两份，取用时再 `l(zh, en)`。
- 前端靠 `set_locale` 命令同步语言，`api.ts` 的每个调用都会等它完成。
- **前端不要拿后端返回的文字做判断**（如 `group === "输入能力"`、`startsWith("原文件已不存在")`），
  这些文字会随语言变。需要判断时让后端加一个稳定字段（如 `ModelField.gid`、`BackupEntry.blockedMissing`）。
- 不翻译：写进用户配置文件的内容（配置键、注释、供应商 id）、日志里给开发者看的调试信息、发给上游 API 的请求内容。
- 测试默认是中文（`i18n` 默认 zh），已有测试里断言的中文文案保持不变。
