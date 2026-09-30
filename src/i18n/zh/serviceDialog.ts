// src/components/ServiceDialog.tsx
import type en from "../en/serviceDialog";

const zh: typeof en = {
  hintResponses: "OpenAI Responses 接口（/v1/responses），Codex 只支持这种",
  blockedGateway: "{agent} 只支持 Google Gemini 协议，本地网关不能转换成它",
  blockedProto: "{agent} 只支持 {api} 接口；打开「使用本地网关」可以转换后接入",
  addGroupTo: "给「{station}」添加分组",
  editGroup: "编辑分组「{name}」",
  protocol: "协议",
  protoMissing: "{vendor} 没有 {api} 接口；需要的 Agent 可以用本地网关转换",
  tplProtocols: "这个模板支持 {list}；只支持其他协议的 Agent 会自动用对应地址。",
  groupHint: "同一中转站的不同协议、不同密钥各建一个分组。",
  keyStorage: "保存在本机 ~/.agentplus，写入各 Agent 时按它们自己的格式保存。",
  commonModels: "常用模型",
  commonModelsHint: "（添加到 ZCode / MiMo 时作为初始模型列表）",
  noModels: "还没有模型，可以拉取或手动添加，之后也能在各 Agent 的「模型列表」里改。",
  syncTo: "同步到这个分组已接入的 Agent",
  syncHint: "（地址、协议或密钥有改动时才需要）",
  addTo: "添加到",
  gatewayOn: "保存后自动开启网关并为它建一条转发，勾选的 Agent 都指向网关地址：协议自动转换（Codex、Claude Code 也能用），密钥只存在供应商库。",
  gatewayOff: "关闭：勾选的 Agent 直接连接上面的地址。",
  sessionHint: "关闭：勾选的 Agent 直接连接。{vendor} 要求 Agent 自己在请求里带会话 ID（x-opencode-session），不带会被拒绝。如果 Agent 不带，请打开「使用本地网关」，网关会自动补上。{more}",
  needsApi: "· 需 {api}",
  convertsTo: "· 转 {api}",
  usesAltUrl: "· 用 {api} 地址",
  noneRequired: "都不勾也可以，只保存到供应商库，之后随时添加。",
  footNote: "供应商库立即保存；写入 Agent 的部分会先进入「待写入的改动」。",
  importSame: "你已经添加过这个供应商（地址、协议和 API Key 都相同）。保存会更新它，而不是再添加一份。",
  importReplace: "保存后，这个分组的 API Key 会换成链接里的；「同步到这个分组已接入的 Agent」里勾选的 Agent 也会换成新 Key。",
  importOthers: "这个地址已经用另一个 API Key 添加过。保存会新建一个分组；如果想把原来的 Key 换成链接里的，请选择要替换的分组：",
  replaceKey: "替换「{name}」的 Key",
};

export default zh;
