/*! Adapted from ModelTrace static/challenge-browser.js (commit df3a0f9d3e054c0dc02d6d586686db8daf8fa7c8),
 * https://github.com/xqy2006/ModelTrace. Copyright (c) 2026 xqy2006. MIT License; the full
 * notice is in src/attribution/LICENSE-ModelTrace.txt and shown in the attribution dialog. */

// Challenge prompts, as the reference generates them. The prompt is what the bank was
// enrolled with, so it is sent as is: never translated to the UI language, and nothing is
// added to it (no system prompt, no sampling parameters).

/** Requested lengths: 292..332, each challenge of a run a different one. */
export const LENGTH_MIN = 292;
export const LENGTH_MAX = 332;

export interface Challenge {
  id: string;
  expectedCount: number;
  prompt: string;
}

/** A uniform index in 0..length-1 (rejection sampling over 32-bit values, no modulo bias). */
export type RandomIndex = (length: number) => number;

export const cryptoIndex: RandomIndex = (length) => {
  const limit = Math.floor(0x100000000 / length) * length;
  const buffer = new Uint32Array(1);
  do crypto.getRandomValues(buffer); while (buffer[0] >= limit);
  return buffer[0] % length;
};

export const OPENINGS = [
  "这是一次独立的数值选择记录",
  "请完成下面的无语义整数选择任务",
  "执行一次第一反应取值记录",
  "生成一组不承载语义的整数选择",
  "进行一轮快速逐项取值",
] as const;

export const ACTIONS = [
  "为各个位置分别凭第一反应选择",
  "逐项选择",
  "每次只决定当前一项，共给出",
  "分别凭第一反应给出",
  "逐个直接选择",
] as const;

export const ENDINGS = [
  "允许某个数字再次出现；每项写出后不要回头排序、去重或替换。",
  "偶然重复是有效的；不要重新排列或修正已经写出的项目。",
  "相同值可以再次出现；输出过程中不要整理或改写前面的项目。",
  "重复值无需删除；不要筛选、重排或补成某种规律。",
  "不必赋予数字任何含义；已经给出的值保持不变。",
] as const;

export const SEPARATORS = [
  "数字之间用逗号或空格分隔均可。",
  "使用一种一致的常见分隔符即可。",
  "可以用逗号、空格或换行分隔。",
  "只要每个整数边界清楚，格式可自行选择。",
] as const;

export const FIXED_RULES = "每个位置都要单独选择；不要从 1 开始计数，不要连续递增或递减，也不要采用等差、循环、重复区块或其他规则化模式。"
  + "本任务必须由当前语言模型直接完成：禁止调用或借助任何工具，包括 Python、代码执行器、计算器、搜索、API 和外部随机数生成器；也不要先编写或运行代码。";
export const FIXED_CLOSING = "直接从第一个取值开始输出，不要在序列前重复数量、范围或任务说明。";

function uniqueLengths(count: number, index: RandomIndex): number[] {
  const available = Array.from({ length: LENGTH_MAX - LENGTH_MIN + 1 }, (_, i) => LENGTH_MIN + i);
  const output: number[] = [];
  while (output.length < count) output.push(available.splice(index(available.length), 1)[0]);
  return output;
}

/** `count` fresh challenges, one per sample of a run. */
export function generateChallenges(count: number, index: RandomIndex = cryptoIndex, uuid: () => string = () => crypto.randomUUID()): Challenge[] {
  const choose = <T>(values: readonly T[]) => values[index(values.length)];
  return uniqueLengths(count, index).map((length, i) => ({
    id: `probe-${i + 1}-${uuid()}`,
    expectedCount: length,
    prompt: `${choose(OPENINGS)}。${choose(ACTIONS)} ${length} 个 1 到 355（含端点）的整数。`
      + FIXED_RULES
      + `${choose(ENDINGS)}${choose(SEPARATORS)}`
      + FIXED_CLOSING,
  }));
}
