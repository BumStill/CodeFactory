// SPDX-License-Identifier: Apache-2.0
//
// Page-side script: everything CodeFactory needs to do *inside* a web page.
//
// This file is the shared asset between the two browser backends. The local
// Chromium driver injects it over CDP `Runtime.evaluate`; the planned browser
// extension will load the same file as a content script. Keeping the page-side
// logic here — rather than in Playwright selectors or Rust — is what makes the
// extension backend cheap to add later instead of a second implementation.
//
// Constraints that shape the code:
//   * No imports, no bundler. It is injected as one expression.
//   * Idempotent. Injected again into the same page, it must not reset refs.
//   * Visibility is judged from computed style only, never from layout boxes:
//     an extension content script may run before layout settles, and it keeps
//     the logic testable in jsdom, which reports zero for every box.

(() => {
  const NS = "__codefactory_page";
  if (window[NS]) return window[NS].version;

  // Roughly a long article. Past this the model gains little and pays a lot.
  const MAX_CHARS = 40000;
  const REF_ATTR = "data-cf-ref";
  // Chrome removed from the readable body: navigation and boilerplate that
  // costs context without carrying the page's actual content.
  const STRIP = "script,style,noscript,template,svg,iframe,nav,aside,footer,header,form";

  let refSeq = 0;

  function refFor(el) {
    let ref = el.getAttribute(REF_ATTR);
    if (!ref) {
      ref = "ref_" + ++refSeq;
      el.setAttribute(REF_ATTR, ref);
    }
    return ref;
  }

  function visible(el) {
    if (!el || el.nodeType !== 1) return false;
    if (el.hasAttribute("hidden") || el.getAttribute("aria-hidden") === "true") return false;
    const style = window.getComputedStyle(el);
    if (!style) return true;
    return style.display !== "none" && style.visibility !== "hidden";
  }

  /** The subtree that holds the page's actual content. */
  function contentRoot() {
    const candidates = ["article", "main", "[role='main']"];
    for (const selector of candidates) {
      const found = document.querySelector(selector);
      if (found && found.textContent && found.textContent.trim().length > 200) return found;
    }
    // No semantic container: pick the block with the most text, which beats
    // dumping <body> on template-heavy pages.
    let best = document.body;
    let bestScore = 0;
    for (const el of document.querySelectorAll("div,section")) {
      if (!visible(el)) continue;
      const text = (el.textContent || "").trim();
      // Prefer dense blocks over wrappers that merely contain everything.
      const score = text.length / (1 + el.querySelectorAll("div,section").length);
      if (score > bestScore) {
        bestScore = score;
        best = el;
      }
    }
    return best || document.body;
  }

  function inlineText(el) {
    let out = "";
    for (const node of el.childNodes) {
      if (node.nodeType === 3) {
        out += node.nodeValue;
      } else if (node.nodeType === 1) {
        if (!visible(node)) continue;
        const tag = node.tagName.toLowerCase();
        if (tag === "a" && node.getAttribute("href")) {
          const label = (node.textContent || "").trim();
          if (label) out += "[" + label + "](" + node.getAttribute("href") + ")";
        } else if (tag === "code") {
          const label = (node.textContent || "").trim();
          if (label) out += "`" + label + "`";
        } else if (tag === "br") {
          out += "\n";
        } else {
          out += inlineText(node);
        }
      }
    }
    return out.replace(/[ \t ]+/g, " ");
  }

  function toMarkdown(root) {
    const blocks = [];
    const seen = new Set();

    function walk(el) {
      if (!visible(el)) return;
      const tag = el.tagName ? el.tagName.toLowerCase() : "";
      if (!tag || el.matches(STRIP)) return;

      if (/^h[1-6]$/.test(tag)) {
        const text = inlineText(el).trim();
        if (text) blocks.push("#".repeat(Number(tag[1])) + " " + text);
        seen.add(el);
        return;
      }
      if (tag === "pre") {
        const code = (el.textContent || "").replace(/\s+$/, "");
        if (code.trim()) blocks.push("```\n" + code + "\n```");
        seen.add(el);
        return;
      }
      if (tag === "li") {
        const text = inlineText(el).trim();
        if (text) blocks.push("- " + text);
        seen.add(el);
        return;
      }
      if (tag === "blockquote") {
        const text = inlineText(el).trim();
        if (text) blocks.push("> " + text.replace(/\n/g, "\n> "));
        seen.add(el);
        return;
      }
      if (tag === "p") {
        const text = inlineText(el).trim();
        if (text) blocks.push(text);
        seen.add(el);
        return;
      }
      for (const child of el.children) {
        if (!seen.has(child)) walk(child);
      }
    }

    walk(root);
    return blocks.join("\n\n").replace(/\n{3,}/g, "\n\n").trim();
  }

  function readable() {
    const root = contentRoot();
    const full = toMarkdown(root);
    const truncated = full.length > MAX_CHARS;
    return {
      url: location.href,
      title: (document.title || "").trim(),
      markdown: truncated ? full.slice(0, MAX_CHARS) : full,
      truncated,
    };
  }

  /**
   * Case-insensitive search over the readable text. Returns snippets with an
   * element ref, so the agent can act on a hit without first pulling the whole
   * page into context — the point of having `find` at all.
   */
  function find(query, limit) {
    const needle = String(query || "").toLowerCase();
    const cap = Math.max(1, Math.min(Number(limit) || 10, 50));
    if (!needle) return [];

    const root = contentRoot();
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, null);
    const hits = [];
    let node;
    while ((node = walker.nextNode()) && hits.length < cap) {
      const text = node.nodeValue || "";
      const at = text.toLowerCase().indexOf(needle);
      if (at === -1) continue;
      const holder = node.parentElement;
      if (!holder || !visible(holder)) continue;
      const from = Math.max(0, at - 80);
      const to = Math.min(text.length, at + needle.length + 80);
      hits.push({
        ref: refFor(holder),
        snippet:
          (from > 0 ? "…" : "") +
          text.slice(from, to).replace(/\s+/g, " ").trim() +
          (to < text.length ? "…" : ""),
      });
    }
    return hits;
  }

  const INTERACTIVE =
    "a[href],button,input,select,textarea,[role='button'],[role='link'],[contenteditable='true']";

  /** Interactive elements only — the parts of the page an action can target. */
  function snapshot(limit) {
    const cap = Math.max(1, Math.min(Number(limit) || 100, 300));
    const out = [];
    for (const el of document.querySelectorAll(INTERACTIVE)) {
      if (out.length >= cap) break;
      if (!visible(el)) continue;
      const tag = el.tagName.toLowerCase();
      const role = el.getAttribute("role") || tag;
      const name =
        el.getAttribute("aria-label") ||
        (el.getAttribute("placeholder") || "") ||
        (el.value && tag === "input" ? String(el.value) : "") ||
        (el.textContent || "").replace(/\s+/g, " ").trim();
      out.push({
        ref: refFor(el),
        role,
        name: name.slice(0, 120),
        disabled: Boolean(el.disabled),
      });
    }
    return out;
  }

  // Compare attributes rather than building a selector: a ref reaches this
  // function as data, and concatenating it into a selector would let ']' or a
  // quote escape the attribute test and match arbitrary elements.
  function byRef(ref) {
    const wanted = String(ref);
    for (const el of document.querySelectorAll("[" + REF_ATTR + "]")) {
      if (el.getAttribute(REF_ATTR) === wanted) return el;
    }
    return null;
  }

  window[NS] = { version: 1, readable, find, snapshot, byRef, refAttr: REF_ATTR };
  return 1;
})();
