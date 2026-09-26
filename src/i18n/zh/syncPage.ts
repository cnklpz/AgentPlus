// src/components/SyncPage.tsx
import type en from "../en/syncPage";

const zh: typeof en = {
  folderSaved: "同步文件夹已保存",
  upToDate: "同步文件和本机一致，没有需要导入的",
  title: "多设备同步",
  intro: "选一个会被同步的文件夹（网盘、NAS、U 盘都可以）。导出时写入供应商和模型列表，不含任何密钥；另一台设备导入后，只需要补填密钥。",
  folder: "同步文件夹",
  folderPlaceholder: "例如 D:\\OneDrive\\AgentPlus",
  fileFromMachine: "文件夹里有同步文件：来自 {machine}，导出于 {time}",
  fileExported: "文件夹里有同步文件：导出于 {time}",
  unknownTime: "未知时间",
  noFile: "文件夹里还没有同步文件。",
  noFolder: "还没有设置同步文件夹。",
  sync: "同步",
  exportLabel: "导出本机配置",
  exportDesc: "把三个 Agent 的供应商和模型列表写到同步文件夹（覆盖旧的同步文件）。",
  export: "导出",
  importLabel: "从同步文件导入",
  importDesc: "和本机对比，列出缺少的供应商和模型。选中的会加入对应 Agent 的「待写入的改动」，确认后再应用。",
  compare: "对比",
  importable: "可以导入的内容",
  noKeyHint: "新增的供应商没有密钥，导入后在供应商详情里点「编辑」补上。",
  adopt: "加入待写入（{n}）",
};

export default zh;
