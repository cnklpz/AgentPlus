// src/components/McpDialog.tsx
import type en from "../en/mcpDialog";

const zh: typeof en = {
  addTitle: "添加 MCP 服务器",
  editTitle: "编辑「{name}」",
  fromLink: "已按导入链接填好（{n} 个服务器）。确认内容、勾选要写入的 Agent 后再添加。",
  pasteConfig: "粘贴配置",
  pasteHint: "README 里的片段、任意 Agent 的配置（JSON、TOML 或 YAML），或 ccswitch:// 的 MCP 导入链接",
  readPaste: "识别",
  pickParsed: "识别到多个，填入：",
  unnamed: "（无名称）",
  transport: "传输方式",
  command: "命令",
  cwd: "工作目录",
  optional: "可选",
  args: "参数",
  onePerLine: "每行一个",
  env: "环境变量",
  url: "地址",
  headers: "请求头",
  varsHint: "变量写成 ${NAME}，写入时会换成各 Agent 自己的写法。隐藏的值（••••）保持原值不变。",
  writeTo: "写入到",
  writeHint: "Agent 的改动在应用后生效；MCP 库会立即保存。",
  replaces: "会替换已有的同名服务器",
  removes: "将被移除",
  needName: "请填写名称",
  needCommand: "请填写命令",
  needUrl: "请填写地址",
  noTransport: "不支持 {transport}",
  noCwd: "不支持工作目录",
};

export default zh;
