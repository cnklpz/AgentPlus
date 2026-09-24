import type zh from "../zh/providerTest";

const en: typeof zh = {
  noSource: "No base URL and API key available",
  title: "Test provider",
  hint: "Sends one short real request (thinking off), using a few tokens",
  modelPlaceholder: "Model ID",
  modelLabel: "Model to test with",
  testing: "Testing…",
  failed: "Test failed",
  ok: "Working",
  notOk: "Not working",
  reply: "Model replied: {reply}",
  noText: "Request succeeded, but the model returned no text (reasoning models may have spent the budget on thinking)",
  tokens: "{input} → {output} tokens",
};

export default en;
