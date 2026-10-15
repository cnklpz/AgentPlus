// AgentPlus: Chinese UI for Antigravity. Evaluated in each Antigravity window (see agzh.rs),
// which fills in the version and the dictionary. Replaces whole text nodes and a few
// attributes that match the dictionary, and keeps doing so as the UI re-renders.
// Conversations, code, editors and terminals are left alone, and so are the names users give
// (conversation titles, projects, files). Evaluating it again with the same version does
// nothing; another version undoes this one first.
(() => {
  const VER = "__VER__";
  const prev = window.__agpZh;
  if (prev && prev.ver === VER) return "same";
  if (prev) prev.off();
  const D = __DICT__;
  const exact = new Map(Object.entries(D.exact));
  // [regex, capture names, output]
  const compile = (list) => list.map(([src, names, out]) => [new RegExp("^" + src + "$"), names, out]);
  const pats = compile(D.patterns);
  // Relative times ("14d", "{x} ago"): only where Antigravity shows a timestamp, since "2d"
  // or "1m" elsewhere can be a name or a count.
  const times = compile(D.times);
  const TIME = "[data-screenshot-volatile]";
  const SKIP = "script,style,code,pre,textarea,input,[contenteditable],[data-quotable],[data-quotable-inline],.select-text,.monaco-editor,.xterm,[data-agp-no-zh]";
  // What users name: only the app's own placeholders in these slots are translated.
  const NAMED = "[data-testid=conversation-row-sidebar] .truncate,[data-testid=project-header] .truncate,[data-testid=breadcrumb-segment],[data-uri]";
  const NAMED_OK = new Set(["Untitled Conversation", "New Conversation"]);
  const ATTRS = ["placeholder", "title", "aria-label"];
  // node → [original, translated]; element → { attr: [original, translated] }
  const texts = new WeakMap();
  const attrs = new WeakMap();

  const match = (k, list, depth, timed) => {
    for (const [re, names, out] of list) {
      const m = re.exec(k);
      if (!m) continue;
      return out.replace(/\{(\w+)\}/g, (_, n) => {
        const c = m[names.indexOf(n) + 1] ?? "";
        // Free text inside a pattern ("{x} ago" with "14d") is looked up once more.
        return n === "n" || n === "m" || depth > 0 ? c : (tr(c, 1, timed) ?? c).trim();
      });
    }
    return undefined;
  };

  // The translation of `s` (surrounding spaces kept), or null.
  const tr = (s, depth, timed) => {
    const k = s.trim();
    if (!k || k.length > 400) return null;
    let v = exact.get(k);
    if (v === undefined && timed) v = match(k, times, depth, timed);
    if (v === undefined) v = match(k, pats, depth, timed);
    if (v === undefined) return null;
    return s.match(/^\s*/)[0] + v + s.match(/\s*$/)[0];
  };

  const translate = (el, s) => {
    if (el.closest(NAMED) && !NAMED_OK.has(s.trim())) return null;
    return tr(s, 0, !!el.closest(TIME));
  };

  const doText = (n) => {
    const cur = n.nodeValue;
    const rec = texts.get(n);
    if (rec && rec[1] === cur) return;
    const el = n.parentElement;
    if (!el || el.closest(SKIP)) return;
    const out = translate(el, cur);
    if (out === null || out === cur) {
      texts.delete(n);
      return;
    }
    texts.set(n, [cur, out]);
    n.nodeValue = out;
  };

  const doAttrs = (el) => {
    let rec = attrs.get(el);
    for (const a of ATTRS) {
      const cur = el.getAttribute(a);
      if (cur === null || (rec && rec[a] && rec[a][1] === cur)) continue;
      // The message box itself is editable; its placeholder is still the app's text.
      if (a === "placeholder" ? el.closest("[data-quotable],.select-text,.monaco-editor,.xterm") : el.closest(SKIP)) continue;
      const out = translate(el, cur);
      if (out === null || out === cur) continue;
      rec = rec || {};
      rec[a] = [cur, out];
      attrs.set(el, rec);
      el.setAttribute(a, out);
    }
  };

  const walk = (root) => {
    if (root.nodeType === 3) return doText(root);
    if (root.nodeType !== 1 && root.nodeType !== 9 && root.nodeType !== 11) return;
    if (root.nodeType === 1) doAttrs(root);
    const w = document.createTreeWalker(root, NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT);
    for (let n = w.nextNode(); n; n = w.nextNode()) {
      if (n.nodeType === 3) doText(n);
      else doAttrs(n);
    }
  };

  const mo = new MutationObserver((list) => {
    for (const m of list) {
      if (m.type === "characterData") doText(m.target);
      else if (m.type === "attributes") doAttrs(m.target);
      else m.addedNodes.forEach(walk);
    }
  });
  mo.observe(document.documentElement, { childList: true, subtree: true, characterData: true, attributes: true, attributeFilter: ATTRS });
  walk(document.documentElement);

  window.__agpZh = {
    ver: VER,
    off() {
      mo.disconnect();
      const w = document.createTreeWalker(document.documentElement, NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT);
      for (let n = w.currentNode; n; n = w.nextNode()) {
        if (n.nodeType === 3) {
          const r = texts.get(n);
          if (r && n.nodeValue === r[1]) n.nodeValue = r[0];
        } else {
          const r = attrs.get(n);
          if (r) for (const a in r) if (n.getAttribute(a) === r[a][1]) n.setAttribute(a, r[a][0]);
        }
      }
      delete window.__agpZh;
    },
  };
  return "on";
})()
