// src/components/McpPage.tsx
import type en from "../en/mcpPage";

const zh: typeof en = {
  title: "MCP 服务器",
  subtitle: "读取各 Agent 的全局配置，不含项目级服务器。",
  empty: "各 Agent 的全局配置里还没有 MCP 服务器。",
  agentError: "{agent}：{error}",
  differs: "定义不一致",
  differsTitle: "各 Agent 里的定义不一样",
  differsHint: "各 Agent 运行这个服务器的方式不同，下面按定义分别列出，并标出使用它的 Agent。",
  detailTitle: "MCP 服务器详情",
  overview: "概览",
  overviewHint: "点左侧的服务器，查看各 Agent 里的定义。",
  statServers: "个服务器",
  statAgents: "个 Agent 有配置",
  statDiffer: "个不一致",
  serverCount: "{n} 个服务器",
  unsupported: "不支持 MCP",
  noFile: "还没有配置文件",
  readFailed: "配置读取失败",
  on: "已启用",
  off: "已停用",
  stashed: "已停用（AgentPlus 暂存）",
  inAgents: "在 {n} 个 Agent 中",
  variant: "定义 {i}/{n}",
  transport: "传输方式",
  command: "命令",
  url: "地址",
  cwd: "工作目录",
  env: "环境变量",
  headers: "请求头",
  otherFields: "{agent} 专有字段",
  masked: "密钥，已隐藏",
};

export default zh;
