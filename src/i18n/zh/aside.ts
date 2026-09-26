// src/components/Aside.tsx
import type en from "../en/aside";

const zh: typeof en = {
  aria: "配置与改动",
  current: "当前配置",
  fromFiles: "读取自配置文件",
  upToDate: "配置已是最新",
  upToDateHint: "在左侧修改后，这里实时列出将写入的内容",
  footRestart: "写入前自动备份原文件 · 应用后点「重启 {name}」生效",
  footStart: "写入前自动备份原文件 · 下次启动 {name} 时生效",
  launch: "启动方式",
  notRunning: "未运行",
  launchUnknown: "正在运行（没找到主进程）",
  byAgentplus: "由 AgentPlus 启动",
  byAgentplusUi: "由 AgentPlus 启动 · 界面增强已生效",
  notByAgentplus: "不是从 AgentPlus 启动的",
  uiInactive: "界面增强（Fast、完整模型名等）要在 AgentPlus 里重启 {name} 才会生效",
  footNewSession: "写入前自动备份原文件 · 新开的 {name} 会话就会读取",
};

export default zh;
