// SPDX-License-Identifier: Apache-2.0
import { describe, it, expect } from "vitest";
import { useEffect } from "react";
import { act, render } from "@testing-library/react";
import { useStickyAutoScroll } from "./useStickyAutoScroll";

// ── Test harness ─────────────────────────────────────────────────────────────
//
// We mount a real component that wires the hook's scrollerRef to a div the
// same way MessageList does. This matches the production timing — ref is
// attached during initial render, BEFORE useEffect/useLayoutEffect fire —
// instead of fighting it the way an after-the-fact `result.current.ref =`
// assignment would.
//
// jsdom doesn't lay things out, so we expose handles for the test to drive
// scrollHeight / clientHeight / scrollTop directly on the div via
// Object.defineProperty. The hook reads them as if the layout engine had
// produced them.

interface HarnessHandle {
  scroller: HTMLDivElement;
  pinned: () => boolean;
  jumpToBottom: () => void;
}

function Harness(props: {
  conversationKey: string | null;
  onMount: (h: HarnessHandle) => void;
  pinnedRef: { current: boolean };
}) {
  const { scrollerRef, pinned, jumpToBottom } = useStickyAutoScroll(props.conversationKey);
  // Keep an external mirror of `pinned` so the test handle's `pinned()` getter
  // returns the up-to-date value across re-renders (not a stale closure value
  // captured at first render).
  props.pinnedRef.current = pinned;

  // Expose the handle ONCE after the ref has been attached. useEffect runs
  // after the DOM commit, so scrollerRef.current is non-null here.
  useEffect(() => {
    if (scrollerRef.current) {
      props.onMount({
        scroller: scrollerRef.current,
        pinned: () => props.pinnedRef.current,
        jumpToBottom,
      });
    }
    // We want to expose exactly once per mount. jumpToBottom is stable
    // (wrapped in useCallback by the hook), so this dep list is fine.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return <div ref={scrollerRef} data-testid="scroller" />;
}

function setLayout(
  el: HTMLElement,
  opts: { scrollHeight?: number; clientHeight?: number; scrollTop?: number },
) {
  if (opts.scrollHeight !== undefined) {
    Object.defineProperty(el, "scrollHeight", { value: opts.scrollHeight, configurable: true });
  }
  if (opts.clientHeight !== undefined) {
    Object.defineProperty(el, "clientHeight", { value: opts.clientHeight, configurable: true });
  }
  if (opts.scrollTop !== undefined) {
    Object.defineProperty(el, "scrollTop", { value: opts.scrollTop, configurable: true, writable: true });
  }
}

async function flushAsync() {
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0));
    await new Promise((r) => setTimeout(r, 0));
    await new Promise((r) => setTimeout(r, 0));
  });
}

interface RenderResult {
  handle: HarnessHandle;
  rerender: (key: string | null) => void;
}

function renderHarness(initialKey: string | null = "session-1"): RenderResult {
  let handle: HarnessHandle | null = null;
  const pinnedRef = { current: true };
  const onMount = (h: HarnessHandle) => { handle = h; };
  const utils = render(<Harness conversationKey={initialKey} onMount={onMount} pinnedRef={pinnedRef} />);
  if (!handle) throw new Error("Harness did not provide handle");
  return {
    handle,
    rerender: (key) =>
      utils.rerender(<Harness conversationKey={key} onMount={onMount} pinnedRef={pinnedRef} />),
  };
}

// ── Tests ────────────────────────────────────────────────────────────────────

describe("useStickyAutoScroll", () => {
  it("snaps to the bottom on initial mount", async () => {
    const { handle } = renderHarness("session-1");
    setLayout(handle.scroller, { scrollHeight: 5000, clientHeight: 600, scrollTop: 0 });
    await flushAsync();
    expect(handle.scroller.scrollTop).toBe(5000);
  });

  it("auto-scrolls when DOM mutations grow the content (streaming tokens)", async () => {
    // The exact bug that re-shipped in v0.3.9: a streaming response mutates
    // text nodes; MutationObserver must fire and pin to the new bottom.
    const { handle } = renderHarness("session-1");
    setLayout(handle.scroller, { scrollHeight: 100, clientHeight: 600, scrollTop: 0 });

    const msg = document.createElement("p");
    msg.textContent = "Hi";
    await act(async () => { handle.scroller.appendChild(msg); });
    await flushAsync();

    // Simulate a fresh token batch arriving. scrollHeight jumps.
    setLayout(handle.scroller, { scrollHeight: 3000, clientHeight: 600 });
    await act(async () => {
      msg.textContent = "Hi, here's the assistant streaming a longer response...";
    });
    await flushAsync();
    expect(handle.scroller.scrollTop).toBe(3000);

    // Another batch — verify continuous following.
    setLayout(handle.scroller, { scrollHeight: 5000, clientHeight: 600 });
    await act(async () => {
      msg.textContent += " ...and continuing past the viewport edge.";
    });
    await flushAsync();
    expect(handle.scroller.scrollTop).toBe(5000);
  });

  it("auto-scrolls when new message blocks are appended", async () => {
    const { handle } = renderHarness("session-1");
    setLayout(handle.scroller, { scrollHeight: 600, clientHeight: 600 });
    await flushAsync();

    setLayout(handle.scroller, { scrollHeight: 1800, clientHeight: 600 });
    await act(async () => {
      const block = document.createElement("div");
      block.textContent = "Brand-new assistant message";
      handle.scroller.appendChild(block);
    });
    await flushAsync();
    expect(handle.scroller.scrollTop).toBe(1800);
  });

  it("stops auto-scrolling once the user scrolls up", async () => {
    const { handle } = renderHarness("session-1");
    setLayout(handle.scroller, { scrollHeight: 3000, clientHeight: 600, scrollTop: 0 });
    await flushAsync();
    expect(handle.scroller.scrollTop).toBe(3000);

    // User scrolls 2000px up — well past the 60px stick threshold.
    await act(async () => {
      Object.defineProperty(handle.scroller, "scrollTop", { value: 1000, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
    });
    expect(handle.pinned()).toBe(false);

    // New content lands. We must NOT yank the user back.
    setLayout(handle.scroller, { scrollHeight: 6000, clientHeight: 600 });
    await act(async () => {
      const block = document.createElement("div");
      block.textContent = "Late-breaking message";
      handle.scroller.appendChild(block);
    });
    await flushAsync();
    expect(handle.scroller.scrollTop).toBe(1000);
  });

  it("re-pins when the user scrolls back near the bottom", async () => {
    const { handle } = renderHarness("session-1");
    setLayout(handle.scroller, { scrollHeight: 3000, clientHeight: 600, scrollTop: 0 });
    await flushAsync();

    // Up
    await act(async () => {
      Object.defineProperty(handle.scroller, "scrollTop", { value: 500, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
    });
    expect(handle.pinned()).toBe(false);

    // Back down within 60px of bottom (3000 - 2400 - 600 = 0)
    await act(async () => {
      Object.defineProperty(handle.scroller, "scrollTop", { value: 2400, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
    });
    expect(handle.pinned()).toBe(true);
  });

  it("snaps to bottom when conversationKey changes (session switch)", async () => {
    // Reproduces the v0.3.7 bug — opening a previous session was landing at
    // the top because pin state leaked across sessions.
    const { handle, rerender } = renderHarness("session-A");
    setLayout(handle.scroller, { scrollHeight: 4000, clientHeight: 600 });
    await flushAsync();

    // User scrolls up in session A
    await act(async () => {
      Object.defineProperty(handle.scroller, "scrollTop", { value: 100, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
    });
    expect(handle.pinned()).toBe(false);

    // Switch to session B
    setLayout(handle.scroller, { scrollHeight: 9000, clientHeight: 600 });
    await act(async () => { rerender("session-B"); });
    await flushAsync();

    expect(handle.scroller.scrollTop).toBe(9000);
    expect(handle.pinned()).toBe(true);
  });

  it("jumpToBottom forces re-pin and snap-to-bottom", async () => {
    const { handle } = renderHarness("session-1");
    setLayout(handle.scroller, { scrollHeight: 5000, clientHeight: 600 });
    await flushAsync();

    // Scroll up
    await act(async () => {
      Object.defineProperty(handle.scroller, "scrollTop", { value: 1000, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
    });
    expect(handle.pinned()).toBe(false);

    await act(async () => { handle.jumpToBottom(); });
    expect(handle.scroller.scrollTop).toBe(5000);
    expect(handle.pinned()).toBe(true);
  });
});
