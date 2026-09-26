// src/components/ProviderTest.tsx
import type en from "../en/providerTest";

const zh: typeof en = {
  noSource: "没有可用的地址和密钥",
  title: "测试供应商",
  hint: "发一条很短的真实请求（关闭思考），会用掉几个 token",
  modelPlaceholder: "模型 ID",
  modelLabel: "测试用的模型",
  testing: "测试中…",
  failed: "测试失败",
  ok: "可用",
  notOk: "不可用",
  reply: "模型回复：{reply}",
  noText: "请求成功，模型没有返回文字（推理模型可能把额度用在了思考上）",
  tokens: "{input} → {output} tokens",
};

export default zh;
