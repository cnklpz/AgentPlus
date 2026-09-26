// src/components/CopyProviderDialog.tsx
import type en from "../en/copyProviderDialog";

const zh: typeof en = {
  globalOpenCode: "全局 OpenCode",
  library: "供应商库",
  ariaTitle: "复制供应商到项目",
  title: "复制供应商到「{name}」",
  intro: "复制地址、密钥和可见的模型，成为这个项目自己的供应商，之后可以单独改模型列表。密钥存进 auth.json，不写进项目文件。",
  filterPlaceholder: "筛选名称或地址",
  empty: "没有可复制的供应商。可以直接「添加供应商」。",
  inherited: "已继承",
  noKey: "无密钥",
  disableInherited: "在这个项目里停用被复制的全局供应商（免得选择器里出现两份）",
  copyN: "复制 {n} 个",
};

export default zh;
