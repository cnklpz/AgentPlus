// src/components/CodexTimezone.tsx
import type en from "../en/codexTimezone";

const zh: typeof en = {
  title: "本地网关",
  toggle: "向模型报告另一个时区",
  desc: "使用本地网关时修改时区。本机时区：{zone}。",
  zoneWithName: "{id}（{name}）",
  label: "时区",
  placeholder: "例如 Asia/Shanghai",
  invalid: "系统不认识这个时区名",
  now: "{name}，当地时间 {time}",
  saved: "时区已设为 {zone}",
  off: "已不再改写时区",
};

export default zh;
