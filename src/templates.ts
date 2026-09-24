import type { ApiKind } from "./api";
import { type TKey, t } from "./i18n";

/** A well-known provider plan: the user only fills in the API key. */
export interface Template {
  id: string;
  /** Vendor icon id (see VendorIcon). */
  icon: string;
  /** Section in the vendor picker. */
  group: string;
  /** Subscription for coding tools, or pay as you go. */
  kind: "plan" | "payg";
  vendor: string;
  plan: string;
  /** Group name in the library. */
  name: string;
  /** Base URL per protocol the plan offers. */
  endpoints: Partial<Record<ApiKind, string>>;
  /** Protocol picked by default (must be in endpoints). */
  api: ApiKind;
  models: string[];
  /** Console page where the key is created. */
  keyUrl: string;
  note?: string;
}

/** Translatable text in the table below: looked up when the field is read, since the
 * language can change at runtime. */
type Text = string | { key: TKey };
const L = (key: TKey): Text => ({ key });
type Lazy = "group" | "vendor" | "plan" | "name" | "note";
type RawTemplate = Omit<Template, Lazy> & { group: Text; vendor: Text; plan: Text; name: Text; note?: Text };

function define(r: RawTemplate): Template {
  const out = { ...r } as unknown as Template;
  for (const f of ["group", "vendor", "plan", "name", "note"] as const) {
    const v = r[f];
    if (v && typeof v === "object") Object.defineProperty(out, f, { get: () => t(v.key), enumerable: true, configurable: true });
  }
  return out;
}

const CN = L("templates.groupCn");
const INTL = L("templates.groupIntl");

// Anthropic bases end in /v1 like the others: AgentPlus appends /messages, and the
// Claude Code adapter strips the /v1 (Claude Code appends /v1/messages itself).
// Checked against each vendor's docs, 2026-09.
export const TEMPLATES: Template[] = ([
  {
    id: "mimo-plan", icon: "mimo", group: CN, kind: "plan", vendor: L("templates.vendorMimo"), plan: "Token Plan", name: "MiMo Token Plan",
    endpoints: { chat: "https://token-plan-cn.xiaomimimo.com/v1", anthropic: "https://token-plan-cn.xiaomimimo.com/anthropic/v1" },
    api: "chat", models: ["mimo-v2.6-pro", "mimo-v2.6-flash", "mimo-v2.5-pro", "mimo-v2.5"],
    keyUrl: "https://platform.xiaomimimo.com/#/console/plan-manage",
    note: L("templates.noteMimoPlan"),
  },
  {
    id: "ark-plan", icon: "volcengine", group: CN, kind: "plan", vendor: L("templates.vendorArk"), plan: "Coding Plan", name: L("templates.nameArkPlan"),
    endpoints: { chat: "https://ark.cn-beijing.volces.com/api/coding/v3", anthropic: "https://ark.cn-beijing.volces.com/api/coding/v1" },
    api: "chat", models: ["ark-code-latest", "doubao-seed-2.1-pro", "doubao-seed-2.1-turbo", "kimi-k3", "glm-5.3", "deepseek-v4-pro", "minimax-m3"],
    keyUrl: "https://ark.volcengine.com/region:cn-beijing/apikey",
    note: L("templates.noteArkPlan"),
  },
  {
    id: "glm-plan", icon: "zhipu", group: CN, kind: "plan", vendor: L("templates.vendorZhipu"), plan: "GLM Coding Plan", name: L("templates.nameGlmPlan"),
    endpoints: { chat: "https://open.bigmodel.cn/api/coding/paas/v4", anthropic: "https://open.bigmodel.cn/api/anthropic/v1" },
    api: "chat", models: ["glm-5.3", "glm-5.3-flash"],
    keyUrl: "https://bigmodel.cn/usercenter/proj-mgmt/apikeys",
    note: L("templates.noteGlmPlan"),
  },
  {
    id: "kimi-code", icon: "kimi", group: CN, kind: "plan", vendor: "Kimi", plan: L("templates.planKimiCode"), name: "Kimi Code",
    endpoints: { chat: "https://api.kimi.com/coding/v1", anthropic: "https://api.kimi.com/coding/v1" },
    api: "chat", models: ["kimi-for-coding"],
    keyUrl: "https://www.kimi.com/code/console",
    note: L("templates.noteKimiCode"),
  },
  {
    id: "bailian-plan", icon: "bailian", group: CN, kind: "plan", vendor: L("templates.vendorBailian"), plan: "Coding Plan", name: L("templates.nameBailianPlan"),
    endpoints: { chat: "https://coding.dashscope.aliyuncs.com/v1", anthropic: "https://coding.dashscope.aliyuncs.com/apps/anthropic/v1" },
    api: "chat", models: ["qwen3.7-plus", "qwen3.6-plus", "qwen3-coder-next", "kimi-k2.5", "glm-5", "MiniMax-M2.5"],
    keyUrl: "https://bailian.console.aliyun.com/",
    note: L("templates.noteBailianPlan"),
  },
  {
    id: "minimax-plan", icon: "minimax", group: CN, kind: "plan", vendor: "MiniMax", plan: "Token Plan", name: "MiniMax Token Plan",
    endpoints: { chat: "https://api.minimax.cn/v1", anthropic: "https://api.minimax.cn/anthropic/v1" },
    api: "chat", models: ["MiniMax-M3", "MiniMax-M2.7", "MiniMax-M2.7-highspeed"],
    keyUrl: "https://platform.minimax.cn/user-center/payment/token-plan",
    note: L("templates.noteMinimaxPlan"),
  },
  {
    id: "tencent-plan", icon: "tencent", group: CN, kind: "plan", vendor: L("templates.vendorTencent"), plan: "Coding Plan", name: L("templates.nameTencentPlan"),
    endpoints: { chat: "https://api.lkeap.cloud.tencent.com/coding/v3", anthropic: "https://api.lkeap.cloud.tencent.com/coding/anthropic/v1" },
    api: "chat", models: ["tc-code-latest"],
    keyUrl: "https://console.cloud.tencent.com/tokenhub/codingplan",
    note: L("templates.noteTencentPlan"),
  },
  {
    id: "qianfan-plan", icon: "qianfan", group: CN, kind: "plan", vendor: L("templates.vendorQianfan"), plan: "Coding Plan", name: L("templates.nameQianfanPlan"),
    endpoints: { chat: "https://qianfan.baidubce.com/v2/coding", anthropic: "https://qianfan.baidubce.com/anthropic/coding/v1" },
    api: "chat", models: ["qianfan-code-latest", "glm-5.1", "deepseek-v4-flash", "kimi-k2.5", "minimax-m2.5"],
    keyUrl: "https://console.bce.baidu.com/qianfan/resource/subscribe",
    note: L("templates.noteQianfanPlan"),
  },

  {
    id: "mimo", icon: "mimo", group: CN, kind: "payg", vendor: L("templates.vendorMimo"), plan: L("templates.planPayg"), name: L("templates.vendorMimo"),
    endpoints: { chat: "https://api.xiaomimimo.com/v1", anthropic: "https://api.xiaomimimo.com/anthropic/v1" },
    api: "chat", models: ["mimo-v2.6-pro", "mimo-v2.6-flash", "mimo-v2.6-pro-ultraspeed", "mimo-v2.5-pro", "mimo-v2.5"],
    keyUrl: "https://platform.xiaomimimo.com/console/api-keys",
    note: L("templates.noteMimo"),
  },
  {
    id: "ark", icon: "volcengine", group: CN, kind: "payg", vendor: L("templates.vendorArk"), plan: L("templates.planPayg"), name: L("templates.vendorArk"),
    endpoints: { chat: "https://ark.cn-beijing.volces.com/api/v3", responses: "https://ark.cn-beijing.volces.com/api/v3" },
    api: "chat", models: ["doubao-seed-2-1-pro-260915", "doubao-seed-2-1-turbo-260628", "doubao-seed-2-1-lite-260915", "doubao-seed-2-0-code-preview-260215", "deepseek-v4-pro-ga-260813"],
    keyUrl: "https://ark.volcengine.com/region:cn-beijing/apikey",
    note: L("templates.noteArk"),
  },
  {
    id: "glm", icon: "zhipu", group: CN, kind: "payg", vendor: L("templates.vendorZhipu"), plan: L("templates.planPayg"), name: L("templates.vendorZhipu"),
    endpoints: { chat: "https://open.bigmodel.cn/api/paas/v4", anthropic: "https://open.bigmodel.cn/api/anthropic/v1" },
    api: "chat", models: ["glm-5.3", "glm-5.2", "glm-5-turbo", "glm-4.7", "glm-4.7-flash"],
    keyUrl: "https://bigmodel.cn/usercenter/proj-mgmt/apikeys",
  },
  {
    id: "kimi", icon: "kimi", group: CN, kind: "payg", vendor: "Kimi", plan: L("templates.planOpenPlatform"), name: L("templates.nameKimiOpen"),
    endpoints: { chat: "https://api.moonshot.cn/v1", anthropic: "https://api.moonshot.cn/anthropic/v1" },
    api: "chat", models: ["kimi-k3", "kimi-k2.7-code", "kimi-k2.7-code-highspeed", "kimi-k2.6"],
    keyUrl: "https://platform.kimi.com/console/api-keys",
  },
  {
    id: "deepseek", icon: "deepseek", group: CN, kind: "payg", vendor: "DeepSeek", plan: L("templates.planPayg"), name: "DeepSeek",
    endpoints: { chat: "https://api.deepseek.com/v1", anthropic: "https://api.deepseek.com/anthropic/v1" },
    api: "chat", models: ["deepseek-v4-pro", "deepseek-flash"],
    keyUrl: "https://platform.deepseek.com/api_keys",
  },
  {
    id: "bailian", icon: "bailian", group: CN, kind: "payg", vendor: L("templates.vendorBailian"), plan: L("templates.planPayg"), name: L("templates.vendorBailian"),
    endpoints: { chat: "https://dashscope.aliyuncs.com/compatible-mode/v1", responses: "https://dashscope.aliyuncs.com/compatible-mode/v1" },
    api: "chat", models: ["qwen3.8-max", "qwen3.7-plus", "qwen3.8-flash", "deepseek-v4-pro-0813", "kimi-k3"],
    keyUrl: "https://bailian.console.aliyun.com/",
    note: L("templates.noteBailian"),
  },
  {
    id: "minimax", icon: "minimax", group: CN, kind: "payg", vendor: "MiniMax", plan: L("templates.planPayg"), name: "MiniMax",
    endpoints: { chat: "https://api.minimax.cn/v1", anthropic: "https://api.minimax.cn/anthropic/v1" },
    api: "chat", models: ["MiniMax-M3", "MiniMax-M2.7", "MiniMax-M2.7-highspeed", "MiniMax-M2.5"],
    keyUrl: "https://platform.minimax.cn/user-center/basic-information/interface-key",
  },
  {
    id: "siliconflow", icon: "siliconflow", group: CN, kind: "payg", vendor: L("templates.vendorSiliconflow"), plan: L("templates.planPayg"), name: L("templates.vendorSiliconflow"),
    endpoints: { chat: "https://api.siliconflow.cn/v1", anthropic: "https://api.siliconflow.cn/v1" },
    api: "chat", models: ["deepseek-ai/DeepSeek-V4-Pro", "Pro/zai-org/GLM-5.1", "Pro/moonshotai/Kimi-K2.6", "Qwen/Qwen3.6-27B"],
    keyUrl: "https://cloud.siliconflow.cn/account/ak",
    note: L("templates.noteSiliconflow"),
  },

  {
    id: "openrouter", icon: "openrouter", group: INTL, kind: "payg", vendor: "OpenRouter", plan: L("templates.planAggregator"), name: "OpenRouter",
    endpoints: { chat: "https://openrouter.ai/api/v1", responses: "https://openrouter.ai/api/v1", anthropic: "https://openrouter.ai/api/v1" },
    api: "chat", models: ["anthropic/claude-opus-5.5", "anthropic/claude-sonnet-5", "openai/gpt-6-sol", "deepseek/deepseek-v4-pro", "moonshotai/kimi-k3", "z-ai/glm-5.3"],
    keyUrl: "https://openrouter.ai/settings/keys",
  },
  {
    id: "openai", icon: "openai", group: INTL, kind: "payg", vendor: "OpenAI", plan: L("templates.planOfficialApi"), name: "OpenAI",
    endpoints: { responses: "https://api.openai.com/v1", chat: "https://api.openai.com/v1" },
    api: "responses", models: ["gpt-6-sol", "gpt-6-astra", "gpt-6-luna"],
    keyUrl: "https://platform.openai.com/api-keys",
  },
  {
    id: "anthropic", icon: "anthropic", group: INTL, kind: "payg", vendor: "Anthropic", plan: L("templates.planOfficialApi"), name: "Anthropic",
    endpoints: { anthropic: "https://api.anthropic.com/v1" },
    api: "anthropic", models: ["claude-opus-5-5", "claude-fable-5-1", "claude-sonnet-5", "claude-haiku-4-5"],
    keyUrl: "https://platform.claude.com/settings/keys",
  },
] satisfies RawTemplate[]).map(define);

export interface Vendor {
  /** Icon id, shared by all the vendor's templates. */
  id: string;
  name: string;
  group: string;
  /** Coding plan first, then pay as you go. */
  plans: Template[];
}

/** Templates by vendor, in the order they first appear. */
// Name and group are getters so they follow the UI language.
export const VENDORS: Vendor[] = TEMPLATES.reduce<Vendor[]>((out, tpl) => {
  const v = out.find((x) => x.id === tpl.icon);
  if (v) v.plans.push(tpl);
  else out.push({ id: tpl.icon, get name() { return tpl.vendor; }, get group() { return tpl.group; }, plans: [tpl] });
  return out;
}, []);

/** Short name of a plan in the plan switch. */
export const planLabel = (tpl: Template) => (tpl.kind === "payg" ? t("templates.planPayg") : tpl.plan);
