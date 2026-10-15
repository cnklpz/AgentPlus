// @vitest-environment jsdom
// The script AgentPlus evaluates in Antigravity's windows for its Chinese UI
// (src-tauri/src/agzh.js; the backend fills in the version and the dictionary).
import { afterEach, describe, expect, it } from "vitest";
import source from "../src-tauri/src/agzh.js?raw";

type Pattern = [string, string[], string];
interface Dict { exact: Record<string, string>; patterns: Pattern[]; times: Pattern[] }

// The same shape agzh.rs builds: `{n}`/`{m}` capture digits, other names any text.
const DICT: Dict = {
  exact: {
    "Settings": "设置",
    "New Conversation": "新建对话",
    "Untitled Conversation": "未命名对话",
    "Review": "审查",
    "Ask anything": "随便问",
    "More options": "更多选项",
  },
  patterns: [
    ["(\\d+) files changed", ["n"], "{n} 个文件已更改"],
    ["Open conversation (.+?)", ["x"], "打开对话 {x}"],
  ],
  times: [
    ["(\\d+)d", ["n"], "{n} 天"],
    ["(\\d+)m", ["n"], "{n} 分钟"],
    ["(.+?) ago", ["x"], "{x}前"],
  ],
};

declare global {
  interface Window { __agpZh?: { ver: string; off(): void } }
}

function run(ver = "v1", dict: Dict = DICT): unknown {
  const code = source.replace("__VER__", ver).replace("__DICT__", () => JSON.stringify(dict));
  return (0, eval)(code);
}

/** Lets the MutationObserver run. */
const settle = () => new Promise((r) => setTimeout(r, 0));
const $ = (sel: string) => document.querySelector(sel)!;

afterEach(() => {
  window.__agpZh?.off();
  document.body.innerHTML = "";
});

describe("agzh.js", () => {
  it("translates whole text nodes and attributes, keeping the spaces around them", () => {
    document.body.innerHTML = `<nav><span id="a"> Settings </span><button id="b" title="More options" aria-label="Review">x</button>
      <div id="c">Settings and more</div><div id="d">3 files changed</div><div id="e">Open conversation Review</div></nav>`;
    expect(run()).toBe("on");
    expect($("#a").textContent).toBe(" 设置 ");
    expect($("#b").getAttribute("title")).toBe("更多选项");
    expect($("#b").getAttribute("aria-label")).toBe("审查");
    // Only whole texts: a sentence that merely contains a key stays.
    expect($("#c").textContent).toBe("Settings and more");
    expect($("#d").textContent).toBe("3 个文件已更改");
    // Free text in a pattern is looked up once more.
    expect($("#e").textContent).toBe("打开对话 审查");
  });

  it("leaves conversations, code, editable text and user-named items alone", () => {
    document.body.innerHTML = `
      <div data-quotable="true"><p id="chat">Settings</p></div>
      <div class="select-text"><span id="md">Review</span></div>
      <pre><code id="code">Settings</code></pre>
      <div contenteditable="true"><span id="edit">Settings</span></div>
      <span data-quotable-inline="true"><span id="mention">Review</span></span>
      <div data-testid="conversation-row-sidebar"><span class="truncate" id="title">Review</span><span class="truncate" id="untitled">Untitled Conversation</span></div>
      <button data-testid="project-header"><span class="truncate" id="project">Settings</span></button>
      <span data-testid="breadcrumb-segment"><span id="crumb">Review</span></span>
      <div contenteditable="true" placeholder="Ask anything" id="box"></div>`;
    run();
    for (const id of ["chat", "md", "code", "edit", "mention", "title", "project", "crumb"]) {
      expect($(`#${id}`).textContent, id).toMatch(/^(Settings|Review)$/);
    }
    // The app's own placeholder in a user-named slot is still its text.
    expect($("#untitled").textContent).toBe("未命名对话");
    // The message box is editable, but its placeholder is the app's.
    expect($("#box").getAttribute("placeholder")).toBe("随便问");
  });

  it("reads relative times only inside timestamps", () => {
    document.body.innerHTML = `<span data-screenshot-volatile="true" id="t1">14d</span>
      <span data-screenshot-volatile="true" id="t2">14d ago</span><span id="name">2d</span><span id="count">1m</span>`;
    run();
    expect($("#t1").textContent).toBe("14 天");
    expect($("#t2").textContent).toBe("14 天前");
    expect($("#name").textContent).toBe("2d");
    expect($("#count").textContent).toBe("1m");
  });

  it("follows the UI as it changes", async () => {
    document.body.innerHTML = `<div id="root"><span id="a">Settings</span></div>`;
    run();
    const a = $("#a");
    // A re-render puts English back in the same node, or another text.
    a.firstChild!.nodeValue = "Review";
    await settle();
    expect(a.textContent).toBe("审查");
    a.firstChild!.nodeValue = "Something else";
    await settle();
    expect(a.textContent).toBe("Something else");
    // New nodes, and attributes set later.
    $("#root").insertAdjacentHTML("beforeend", `<p id="b"><i>New Conversation</i></p>`);
    $("#a").setAttribute("title", "More options");
    await settle();
    expect($("#b").textContent).toBe("新建对话");
    expect($("#a").getAttribute("title")).toBe("更多选项");
    // Its own changes don't feed back: the text is stable after a few rounds.
    await settle();
    await settle();
    expect($("#b").textContent).toBe("新建对话");
  });

  it("puts the English back when switched off, without undoing the app's own changes", async () => {
    document.body.innerHTML = `<span id="a">Settings</span><span id="b">Review</span><input id="c" placeholder="Ask anything">`;
    run();
    $("#b").firstChild!.nodeValue = "Changed by the app";
    await settle();
    window.__agpZh!.off();
    expect($("#a").textContent).toBe("Settings");
    expect($("#b").textContent).toBe("Changed by the app");
    expect($("#c").getAttribute("placeholder")).toBe("Ask anything");
    expect(window.__agpZh).toBeUndefined();
    // Nothing is translated any more.
    $("#a").firstChild!.nodeValue = "Review";
    await settle();
    expect($("#a").textContent).toBe("Review");
  });

  it("does nothing for the same version and swaps the dictionary for a new one", () => {
    document.body.innerHTML = `<span id="a">Settings</span>`;
    run("v1");
    expect(run("v1")).toBe("same");
    run("v2", { ...DICT, exact: { ...DICT.exact, Settings: "偏好设置" } });
    expect($("#a").textContent).toBe("偏好设置");
    expect(window.__agpZh!.ver).toBe("v2");
  });
});
