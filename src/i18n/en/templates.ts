import type zh from "../zh/templates";

const en: typeof zh = {
  groupCn: "China",
  groupIntl: "Global · aggregators",

  vendorMimo: "Xiaomi MiMo",
  vendorArk: "Volcengine Ark",
  vendorZhipu: "Zhipu",
  vendorBailian: "Alibaba Cloud Bailian",
  vendorTencent: "Tencent Cloud",
  vendorQianfan: "Baidu Qianfan",
  vendorSiliconflow: "SiliconFlow",

  planKimiCode: "Kimi Code membership",
  planPayg: "Pay as you go",
  planOpenPlatform: "Open platform",
  planAggregator: "Aggregator",
  planOfficialApi: "Official API",

  nameArkPlan: "Volcengine Ark Coding Plan",
  nameGlmPlan: "Zhipu GLM Coding Plan",
  nameBailianPlan: "Bailian Coding Plan",
  nameTencentPlan: "Tencent Cloud Coding Plan",
  nameQianfanPlan: "Qianfan Coding Plan",
  nameKimiOpen: "Kimi Open Platform",

  noteMimoPlan: "Plan key (starts with tp- / ttp-); not interchangeable with pay-as-you-go sk- keys",
  noteArkPlan: "Only the dedicated /api/coding URL draws on the plan quota; ark-code-latest follows the model chosen in the console",
  noteGlmPlan: "Only the dedicated /api/coding URL draws on the plan quota",
  noteKimiCode: "kimi-for-coding always follows the latest model; the key is shown only once",
  noteBailianPlan: "The plan key (starts with sk-sp-) is on the Coding Plan page of the Bailian console; not interchangeable with regular sk- keys",
  noteMinimaxPlan: "Same URL as pay as you go; use the key from the plan page",
  noteTencentPlan: "Plan key (starts with sk-sp-); not interchangeable with regular keys",
  noteQianfanPlan: "The plan key only works with the dedicated coding URL",
  noteMimo: "Open platform sk- key; usage is billed in credits",
  noteArk: "Model IDs include a version date; you can also enter an inference endpoint ID from the console",
  noteBailian: "Beijing region; a key only works in the region where it was created",
  noteSiliconflow: "Models starting with Pro/ are the paid versions",
};

export default en;
