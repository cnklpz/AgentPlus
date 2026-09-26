// src/services.ts
import type en from "../en/services";

const zh: typeof en = {
  gatewayStation: "本地网关",
  geminiOnly: "{agent} 只支持 Google Gemini 协议，本地网关也不能转换成它",
  apiOnly: "{agent} 只支持 {only} 接口，这个分组是 {api}（可以通过本地网关转换）",
  geminiGroup: "这是 Gemini 协议的分组，只能用于 Gemini CLI",
  noSource: "没有可复制的地址和密钥，先编辑补全",
  useCurrent: "当前使用",
  useOn: "已接入",
  useAdding: "待添加",
  useRemoving: "待移除",
  useNew: "新 · 未应用",
};

export default zh;
