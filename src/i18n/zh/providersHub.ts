// src/components/ProvidersHub.tsx
import type en from "../en/providersHub";

const zh: typeof en = {
  intro: "按服务地址归成中转站，一个中转站可以有多个分组（不同协议、路径或密钥）。地址和密钥在这里维护，模型在各 Agent 的「模型列表」里设置。当前环境：{env}",
  testLatency: "测试延迟",
  filterAll: "全部",
  filterUsed: "已接入",
  filterIdle: "未使用",
  searchPlaceholder: "按名称、地址、分组搜索",
  notAdded: "未接入",
  agentState: "{agent}：{state}",
  moreGroups: "还有 {n} 个分组",
  groupCount: "{n} 个分组",
  inLibraryTitle: "已保存在供应商库，可添加到任意 Agent",
  inLibrary: "已收录 {n}",
  notInLibraryTitle: "只存在于 Agent 配置里，编辑后会收录进供应商库",
  notInLibrary: "未收录",
  addCardHint: "填一次地址和密钥，按需添加到 {agents}",
  eachAgent: "各 Agent",
  noMatch: "没有匹配的供应商",
  accounts: "账号登录 · 各 Agent 内置",
};

export default zh;
