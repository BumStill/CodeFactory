// SPDX-License-Identifier: Apache-2.0
//
// The page-side script runs inside web pages, so it is tested against a real
// DOM rather than by asserting on its source. jsdom is the honest level for
// this: extraction, search, and ref assignment are pure DOM work, and the
// script deliberately judges visibility from computed style rather than layout
// boxes precisely so it behaves the same here as in Chromium.
//
// This is also the file the browser extension will load as a content script,
// so these tests cover both backends.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { beforeEach, describe, expect, it } from "vitest";

// Vitest serves modules over http, so import.meta.url is not a file URL here;
// resolve from the repo root instead.
const SCRIPT = readFileSync(
  resolve(process.cwd(), "src-tauri/src/browser/page.js"),
  "utf8",
);

interface PageApi {
  version: number;
  readable(): { url: string; title: string; markdown: string; truncated: boolean };
  find(query: string, limit?: number): { ref: string; snippet: string }[];
  snapshot(limit?: number): { ref: string; role: string; name: string; disabled: boolean }[];
  byRef(ref: string): Element | null;
  refAttr: string;
}

/** Inject the script the way the driver does, and hand back its namespace. */
function inject(): PageApi {
  new Function(SCRIPT)();
  return (window as unknown as Record<string, PageApi>).__codefactory_page;
}

function setBody(html: string) {
  document.body.innerHTML = html;
  delete (window as unknown as Record<string, unknown>).__codefactory_page;
}

const ARTICLE = `
  <nav><a href="/home">Home</a><a href="/pricing">Pricing</a></nav>
  <article>
    <h1>Quarterly report</h1>
    <p>Revenue grew by <code>12%</code> across the quarter.</p>
    <p>See the <a href="/appendix">appendix</a> for the breakdown.</p>
    <ul><li>North America up 8%</li><li>Europe up 15%</li></ul>
    <pre>total = 1_200_000</pre>
    <blockquote>Margins held steady.</blockquote>
  </article>
  <footer><a href="/legal">Legal</a></footer>
`;

describe("page script — reading", () => {
  beforeEach(() => setBody(ARTICLE));

  it("extracts the article as markdown and leaves out the chrome around it", () => {
    const page = inject();
    const { markdown } = page.readable();

    expect(markdown).toContain("# Quarterly report");
    expect(markdown).toContain("Revenue grew by `12%`");
    expect(markdown).toContain("- North America up 8%");
    expect(markdown).toContain("> Margins held steady.");
    expect(markdown).toContain("```");

    // Navigation and footer boilerplate cost context and carry no content.
    expect(markdown).not.toContain("Pricing");
    expect(markdown).not.toContain("Legal");
  });

  it("keeps links as markdown so the agent can follow them without a snapshot", () => {
    const page = inject();
    expect(page.readable().markdown).toContain("[appendix](/appendix)");
  });

  it("reports truncation instead of silently dropping the tail", () => {
    setBody(`<article><p>${"word ".repeat(20000)}</p></article>`);
    const page = inject();
    const result = page.readable();

    expect(result.truncated).toBe(true);
    expect(result.markdown.length).toBeLessThanOrEqual(40000);
  });

  it("does not treat a hidden section as page content", () => {
    setBody(`
      <article><p>Visible body text that is long enough to be the content root.</p></article>
      <div style="display:none"><p>secret draft</p></div>
    `);
    const page = inject();
    expect(page.readable().markdown).not.toContain("secret draft");
  });
});

describe("page script — find", () => {
  beforeEach(() => setBody(ARTICLE));

  it("returns a ref and a snippet instead of the whole page", () => {
    const page = inject();
    const hits = page.find("Europe");

    expect(hits).toHaveLength(1);
    expect(hits[0].snippet).toContain("Europe up 15%");
    expect(hits[0].ref).toMatch(/^ref_\d+$/);
    // The ref must resolve back to the element that matched.
    expect(page.byRef(hits[0].ref)?.textContent).toContain("Europe");
  });

  it("matches case-insensitively and honours the limit", () => {
    setBody(`<article>${"<p>alpha beta</p>".repeat(5)}</article>`);
    const page = inject();

    expect(page.find("ALPHA").length).toBeGreaterThan(0);
    expect(page.find("alpha", 2)).toHaveLength(2);
  });

  it("returns nothing for an empty query rather than every text node", () => {
    const page = inject();
    expect(page.find("")).toEqual([]);
  });
});

describe("page script — snapshot and refs", () => {
  it("lists interactive elements with stable refs", () => {
    setBody(`
      <button aria-label="Send message">Send</button>
      <input placeholder="Search articles" />
      <a href="/next">Next page</a>
      <button disabled>Archived</button>
    `);
    const page = inject();
    const snapshot = page.snapshot();

    const names = snapshot.map((entry) => entry.name);
    expect(names).toContain("Send message");
    expect(names).toContain("Search articles");
    expect(names).toContain("Next page");

    // Disabled controls are still listed — the agent needs to know they exist
    // and are unavailable, otherwise it retries a click that cannot work.
    expect(snapshot.find((entry) => entry.name === "Archived")?.disabled).toBe(true);
  });

  it("keeps a ref pointing at the same element across calls", () => {
    setBody(`<button>Only</button>`);
    const page = inject();

    const first = page.snapshot()[0].ref;
    const second = page.snapshot()[0].ref;
    expect(second).toBe(first);
  });

  it("survives being injected twice without renumbering refs", () => {
    // The driver re-injects on every navigation-free call; if that reset the
    // counter, a ref the model is holding would start pointing elsewhere.
    setBody(`<button>Only</button>`);
    const page = inject();
    const before = page.snapshot()[0].ref;

    new Function(SCRIPT)();
    const after = page.snapshot()[0].ref;
    expect(after).toBe(before);
  });

  it("a ref carrying selector metacharacters cannot match another element", () => {
    // byRef compares attributes rather than building a selector, so a hostile
    // ref resolves to nothing instead of widening the match.
    setBody(`<button>One</button><button>Two</button>`);
    const page = inject();
    page.snapshot();

    expect(page.byRef("ref_1'] , button, [x='")).toBeNull();
    expect(page.byRef("ref_1")?.textContent).toBe("One");
  });

  it("skips controls that are hidden from the user", () => {
    setBody(`
      <button>Visible</button>
      <button style="display:none">Hidden</button>
      <button aria-hidden="true">Aria hidden</button>
    `);
    const page = inject();
    const names = page.snapshot().map((entry) => entry.name);

    expect(names).toEqual(["Visible"]);
  });
});

describe("progress listener outside Tauri", () => {
  it("resolves to a no-op instead of throwing into an unhandled rejection", async () => {
    // This exact failure shipped twice: every test file reported passing while
    // vitest exited non-zero, because subscribing without a Tauri runtime
    // rejects asynchronously. A component that renders in jsdom must be able to
    // call this safely.
    const { onChromiumProgress } = await import("./tauri");
    expect("__TAURI_INTERNALS__" in window).toBe(false);

    const unlisten = await onChromiumProgress(() => {});
    expect(typeof unlisten).toBe("function");
    expect(() => unlisten()).not.toThrow();
  });
});
