// src/components/ModelDialog.tsx
import type en from "../en/modelDialog";

const zh: typeof en = {
  editTitle: "编辑模型",
  editHead: "编辑模型 {id}",
  addHead: "添加模型到 {agent}",
  addTitle: "添加模型",
  modelId: "模型 ID",
  modelIdPlaceholder: "例如：deepseek-v4-pro",
  idLocked: "ID 是请求里发给供应商的模型名，不能改；要换 ID 请删掉后重新添加。",
  displayName: "显示名",
  displayNamePlaceholder: "可选，留空显示 ID",
  upstreamModel: "上游模型",
  upstreamModelPlaceholder: "可选，留空时与模型 ID 相同",
  context: "上下文窗口",
  contextPlaceholder: "如 128k / 1m / 200000",
  contextBad: "写成数字，可以带 k / m",
  contextTokens: "{n} tokens（{short}）",
  defaultNote: "标着「默认」的项不写进配置，由 {agent} 自己决定。",
  modified: "已修改",
  default: "默认",
  numberBad: "写成数字，可以带 k",
  resetDefault: "恢复默认",
  matched: "已按「{id}」自动填入（{source}），可以再改",
  sourceBuiltin: "内置资料",
  sourceModelsDev: "models.dev",
  auto: "自动匹配",
  matchButton: "智能匹配",
  matchHint: "按模型 ID 查内置资料和 models.dev，只填还没设置的项",
  matchFilled: "已按「{id}」填入 {n} 项",
  matchNothingNew: "「{id}」的资料里没有可补的项",
  matchNone: "没找到这个模型的资料",
};

export default zh;
