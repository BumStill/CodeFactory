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
  hasNewContent: () => boolean;
  jumpToBottom: () => void;
  prepareForPrepend: ReturnType<typeof useStickyAutoScroll>["prepareForPrepend"];
}

function Harness(props: {
  conversationKey: string | null;
  contentSignal?: string;
  onMount: (h: HarnessHandle) => void;
  pinnedRef: { current: boolean };
  hasNewContentRef: { current: boolean };
}) {
  const {
    scrollerRef,
    pinned,
    hasNewContent,
    jumpToBottom,
    prepareForPrepend,
  } = useStickyAutoScroll(props.conversationKey, props.contentSignal);
  // Mirror live state so the test handle returns up-to-date values across
  // re-renders (not stale closures captured at first render).
  props.pinnedRef.current = pinned;
  props.hasNewContentRef.current = hasNewContent;

  // Expose the handle ONCE after the ref has been attached. useEffect runs
  // after the DOM commit, so scrollerRef.current is non-null here.
  useEffect(() => {
    if (scrollerRef.current) {
      props.onMount({
        scroller: scrollerRef.current,
        pinned: () => props.pinnedRef.current,
        hasNewContent: () => props.hasNewContentRef.current,
        jumpToBottom,
        prepareForPrepend,
      });
    }
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
  const hasNewContentRef = { current: false };
  const onMount = (h: HarnessHandle) => { handle = h; };
  const utils = render(
    <Harness conversationKey={initialKey} onMount={onMount}
             pinnedRef={pinnedRef} hasNewContentRef={hasNewContentRef} />
  );
  if (!handle) throw new Error("Harness did not provide handle");
  return {
    handle,
    rerender: (key) =>
      utils.rerender(
        <Harness conversationKey={key} onMount={onMount}
                 pinnedRef={pinnedRef} hasNewContentRef={hasNewContentRef} />
      ),
  };
}

function renderSignalHarness(initialSignal: string): {
  handle: HarnessHandle;
  rerenderSignal: (contentSignal: string) => void;
} {
  let handle: HarnessHandle | null = null;
  const pinnedRef = { current: true };
  const hasNewContentRef = { current: false };
  const onMount = (value: HarnessHandle) => { handle = value; };
  const renderHarnessWithSignal = (contentSignal: string) => (
    <Harness
      conversationKey="session-1"
      contentSignal={contentSignal}
      onMount={onMount}
      pinnedRef={pinnedRef}
      hasNewContentRef={hasNewContentRef}
    />
  );
  const utils = render(renderHarnessWithSignal(initialSignal));
  if (!handle) throw new Error("Harness did not provide handle");
  return {
    handle,
    rerenderSignal: (contentSignal: string) =>
      utils.rerender(renderHarnessWithSignal(contentSignal)),
  };
}

function DelayedHarness(props: {
  showScroller: boolean;
  conversationKey: string | null;
  onReady: (handle: HarnessHandle) => void;
  pinnedRef: { current: boolean };
  hasNewContentRef: { current: boolean };
}) {
  const {
    scrollerRef,
    pinned,
    hasNewContent,
    jumpToBottom,
    prepareForPrepend,
  } = useStickyAutoScroll(props.conversationKey);
  props.pinnedRef.current = pinned;
  props.hasNewContentRef.current = hasNewContent;

  useEffect(() => {
    if (props.showScroller && scrollerRef.current) {
      props.onReady({
        scroller: scrollerRef.current,
        pinned: () => props.pinnedRef.current,
        hasNewContent: () => props.hasNewContentRef.current,
        jumpToBottom,
        prepareForPrepend,
      });
    }
  }, [props.showScroller, props.conversationKey, jumpToBottom, props, scrollerRef]);

  return props.showScroller ? <div ref={scrollerRef} data-testid="delayed-scroller" /> : null;
}

// ── Tests ────────────────────────────────────────────────────────────────────

describe("useStickyAutoScroll", () => {
  it("detaches on the first upward scroll after an initial programmatic pin", async () => {
    const { handle } = renderHarness("short-overflow");
    setLayout(handle.scroller, {
      scrollHeight: 765,
      clientHeight: 638,
      scrollTop: 0,
    });
    await flushAsync();
    expect(handle.scroller.scrollTop).toBe(765);

    // The first real wheel gesture goes to the top. Its 127px distance is
    // inside the down-scroll catch-up threshold but outside the up-scroll
    // threshold, so direction must be measured from our pinned position.
    await act(async () => {
      Object.defineProperty(handle.scroller, "scrollTop", {
        value: 0,
        configurable: true,
        writable: true,
      });
      handle.scroller.dispatchEvent(new Event("scroll"));
    });

    expect(handle.pinned()).toBe(false);
  });

  it("preserves the reading anchor when older history is prepended", async () => {
    const { handle } = renderHarness("long-session");
    setLayout(handle.scroller, {
      scrollHeight: 4000,
      clientHeight: 600,
      scrollTop: 0,
    });
    await flushAsync();

    await act(async () => {
      Object.defineProperty(handle.scroller, "scrollTop", {
        value: 1200,
        configurable: true,
        writable: true,
      });
      handle.scroller.dispatchEvent(new Event("scroll"));
    });
    expect(handle.pinned()).toBe(false);

    const anchor = handle.prepareForPrepend();
    expect(anchor).not.toBeNull();

    // Eight older turns add 1,500 px above the current viewport. The DOM
    // mutation must not snap the reader to the live tail or claim that the
    // persisted history is fresh streaming content.
    setLayout(handle.scroller, { scrollHeight: 5500 });
    await act(async () => {
      const older = document.createElement("div");
      older.textContent = "eight older persisted turns";
      handle.scroller.prepend(older);
    });
    await flushAsync();
    expect(handle.scroller.scrollTop).toBe(1200);
    expect(handle.hasNewContent()).toBe(false);

    await act(async () => {
      anchor?.restore();
    });

    expect(handle.scroller.scrollTop).toBe(2700);
    expect(handle.pinned()).toBe(false);
    expect(handle.hasNewContent()).toBe(false);
  });

  it("attaches growth tracking when an initially empty conversation mounts its scroller later", async () => {
    let handle: HarnessHandle | null = null;
    const pinnedRef = { current: true };
    const hasNewContentRef = { current: false };
    const onReady = (next: HarnessHandle) => { handle = next; };
    const utils = render(
      <DelayedHarness
        showScroller={false}
        conversationKey={null}
        onReady={onReady}
        pinnedRef={pinnedRef}
        hasNewContentRef={hasNewContentRef}
      />,
    );

    await act(async () => {
      utils.rerender(
        <DelayedHarness
          showScroller
          conversationKey="first-message"
          onReady={onReady}
          pinnedRef={pinnedRef}
          hasNewContentRef={hasNewContentRef}
        />,
      );
    });
    if (!handle) throw new Error("delayed scroller did not mount");
    const ready = handle as HarnessHandle;
    setLayout(ready.scroller, { scrollHeight: 1200, clientHeight: 500, scrollTop: 0 });
    await flushAsync();

    setLayout(ready.scroller, { scrollHeight: 2600 });
    await act(async () => {
      const streamed = document.createElement("p");
      streamed.textContent = "first assistant response keeps streaming";
      ready.scroller.appendChild(streamed);
    });
    await flushAsync();

    expect(ready.scroller.scrollTop).toBe(2600);
    expect(ready.pinned()).toBe(true);

    // The delayed mount must attach the user-scroll listener too, not just
    // perform a one-off snap. Once the user moves up, later tokens stay put.
    await act(async () => {
      Object.defineProperty(ready.scroller, "scrollTop", { value: 600, configurable: true, writable: true });
      ready.scroller.dispatchEvent(new Event("scroll"));
    });
    expect(ready.pinned()).toBe(false);
    setLayout(ready.scroller, { scrollHeight: 3400 });
    await act(async () => {
      const streamed = document.createElement("p");
      streamed.textContent = "more after the reader scrolled up";
      ready.scroller.appendChild(streamed);
    });
    await flushAsync();
    expect(ready.scroller.scrollTop).toBe(600);
  });

  it("keeps the pin-loss bottom stable across a multi-event thumb drag while the stream grows", async () => {
    const { handle } = renderHarness("session-1");
    setLayout(handle.scroller, { scrollHeight: 2000, clientHeight: 500 });
    await flushAsync();

    // Leave the tail. The bottom visible at pin loss is 1500.
    await act(async () => {
      Object.defineProperty(handle.scroller, "scrollTop", { value: 700, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
    });
    expect(handle.pinned()).toBe(false);

    // Drag downward through multiple browser scroll events. New tokens arrive
    // between events, so the live bottom keeps advancing. Updating the baseline
    // on every event makes the target impossible to catch.
    await act(async () => {
      setLayout(handle.scroller, { scrollHeight: 2600 });
      Object.defineProperty(handle.scroller, "scrollTop", { value: 1050, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
      setLayout(handle.scroller, { scrollHeight: 3200 });
      Object.defineProperty(handle.scroller, "scrollTop", { value: 1300, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
      setLayout(handle.scroller, { scrollHeight: 3800 });
      Object.defineProperty(handle.scroller, "scrollTop", { value: 1500, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
    });
    await flushAsync();

    expect(handle.pinned()).toBe(true);
    expect(handle.scroller.scrollTop).toBe(3800);

    setLayout(handle.scroller, { scrollHeight: 4400 });
    await act(async () => {
      const next = document.createElement("p");
      next.textContent = "stream continues";
      handle.scroller.appendChild(next);
    });
    await flushAsync();
    expect(handle.scroller.scrollTop).toBe(4400);
  });

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

  it("ignores the scroll event the browser fires in response to our own programmatic scroll", async () => {
    // The exact bug we shipped in v0.3.16: every programmatic scrollTop
    // write makes the browser dispatch a scroll event. Without filtering
    // it out, onScroll concludes "near bottom → pinned=true", and the
    // moment the user tries to wheel up the next MutationObserver tick
    // snaps them right back. The fix is in lastSetScrollTop + the
    // ignoreScrollUntil window.
    const { handle } = renderHarness("session-1");
    setLayout(handle.scroller, { scrollHeight: 3000, clientHeight: 600, scrollTop: 0 });
    await flushAsync();
    expect(handle.scroller.scrollTop).toBe(3000);

    // Simulate the browser's auto-fired scroll event echoing the value we
    // just set. It must NOT flip pinned (we're at the bottom already, but
    // more importantly the event itself isn't a user signal).
    await act(async () => { handle.scroller.dispatchEvent(new Event("scroll")); });
    expect(handle.pinned()).toBe(true);

    // Now the user wheels up — different scrollTop than the one we wrote.
    // Pinned must flip to false.
    await act(async () => {
      Object.defineProperty(handle.scroller, "scrollTop", { value: 800, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
    });
    expect(handle.pinned()).toBe(false);

    // And the very next MutationObserver tick from streaming content
    // must NOT yank them back to the bottom.
    setLayout(handle.scroller, { scrollHeight: 9000, clientHeight: 600 });
    await act(async () => {
      const node = document.createElement("p");
      node.textContent = "more streaming";
      handle.scroller.appendChild(node);
    });
    await flushAsync();
    expect(handle.scroller.scrollTop).toBe(800);
    expect(handle.pinned()).toBe(false);
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

  it("sets hasNewContent when content grows while user is scrolled up", async () => {
    // Reproduces the v0.3.17 gap: the floating button said "Jump to latest"
    // but gave the user no signal that there was actually new content. The
    // hook now tracks growth-while-unpinned so the UI can show a different,
    // attention-grabbing style.
    const { handle } = renderHarness("session-1");
    setLayout(handle.scroller, { scrollHeight: 3000, clientHeight: 600 });
    await flushAsync();
    expect(handle.hasNewContent()).toBe(false);

    // User scrolls up.
    await act(async () => {
      Object.defineProperty(handle.scroller, "scrollTop", { value: 500, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
    });
    expect(handle.pinned()).toBe(false);
    // Nothing has grown yet — no badge.
    expect(handle.hasNewContent()).toBe(false);

    // Streaming token arrives while the user is still up-top.
    setLayout(handle.scroller, { scrollHeight: 6000, clientHeight: 600 });
    await act(async () => {
      const node = document.createElement("p");
      node.textContent = "fresh streaming text";
      handle.scroller.appendChild(node);
    });
    await flushAsync();

    // Now the badge should be active.
    expect(handle.hasNewContent()).toBe(true);
    // And we must still not have yanked the user back.
    expect(handle.scroller.scrollTop).toBe(500);
  });

  it("clears hasNewContent when the user clicks jump-to-bottom", async () => {
    const { handle } = renderHarness("session-1");
    setLayout(handle.scroller, { scrollHeight: 3000, clientHeight: 600 });
    await flushAsync();

    // Scroll up, then receive new content.
    await act(async () => {
      Object.defineProperty(handle.scroller, "scrollTop", { value: 500, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
    });
    setLayout(handle.scroller, { scrollHeight: 6000, clientHeight: 600 });
    await act(async () => {
      const node = document.createElement("p");
      node.textContent = "more";
      handle.scroller.appendChild(node);
    });
    await flushAsync();
    expect(handle.hasNewContent()).toBe(true);

    // Click jump-to-bottom — should snap to actual current scrollHeight
    // (not the stale "old bottom") AND clear the badge AND re-pin.
    await act(async () => { handle.jumpToBottom(); });
    expect(handle.scroller.scrollTop).toBe(6000);
    expect(handle.pinned()).toBe(true);
    expect(handle.hasNewContent()).toBe(false);
  });

  it("re-pins when the user scrolls down toward a tail that has grown beyond their old position", async () => {
    // The scenario users actually run into during long streams:
    //   1. They scroll up while content is being streamed.
    //   2. The stream keeps growing — scrollHeight is now much further
    //      down than where they stopped.
    //   3. They scroll back DOWN to the latest message.
    //   4. Because the tail kept moving, their scroll position no longer
    //      fits inside a 60px-from-actual-bottom window.
    // With a single tight threshold the user could never re-pin without
    // perfectly hitting the moving target. The fix: a wider stick zone
    // when the scroll direction is downward (catching up to streaming).
    const { handle } = renderHarness("session-1");
    setLayout(handle.scroller, { scrollHeight: 3000, clientHeight: 600 });
    await flushAsync();

    // Scroll up first.
    await act(async () => {
      Object.defineProperty(handle.scroller, "scrollTop", { value: 500, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
    });
    expect(handle.pinned()).toBe(false);

    // Stream balloons the content — scrollHeight grows past their position.
    setLayout(handle.scroller, { scrollHeight: 10000, clientHeight: 600 });
    await act(async () => {
      const block = document.createElement("div");
      block.textContent = "lots of streaming";
      handle.scroller.appendChild(block);
    });
    await flushAsync();
    expect(handle.hasNewContent()).toBe(true);
    expect(handle.scroller.scrollTop).toBe(500); // user untouched

    // User scrolls back down — lands ~150px from current bottom (within
    // the 240px catch-up zone but well outside the old 60px window).
    // Distance: 10000 - 9250 - 600 = 150.
    await act(async () => {
      // First a smaller down-step so the direction detector sees "down"
      // Stay above the bottom that was visible when pinning was lost. Going
      // past that old bottom would intentionally trigger the separate
      // reachedSeenBottom recovery path and snap before this second event.
      Object.defineProperty(handle.scroller, "scrollTop", { value: 1000, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
      Object.defineProperty(handle.scroller, "scrollTop", { value: 9250, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
    });

    // Re-pinned because the down-scroll widened the threshold to 240.
    expect(handle.pinned()).toBe(true);
    expect(handle.hasNewContent()).toBe(false);

    // Next streaming token must auto-snap to the true bottom.
    setLayout(handle.scroller, { scrollHeight: 12000, clientHeight: 600 });
    await act(async () => {
      const block = document.createElement("div");
      block.textContent = "more";
      handle.scroller.appendChild(block);
    });
    await flushAsync();
    expect(handle.scroller.scrollTop).toBe(12000);
  });

  it("does NOT re-pin when scrolling UP into the wider catch-up zone", async () => {
    // The flip side of the previous test: a wider threshold is correct
    // for downward catch-up but would be wrong for upward scrolls — if
    // user wheels up 200px the system shouldn't say "still pinned" and
    // ignore them. Up direction must keep the tight 60px window.
    const { handle } = renderHarness("session-1");
    setLayout(handle.scroller, { scrollHeight: 3000, clientHeight: 600 });
    await flushAsync();
    expect(handle.pinned()).toBe(true);
    expect(handle.scroller.scrollTop).toBe(3000);

    // Wheel up by 200px — within the down-direction 240px zone but
    // clearly "the user left the bottom" if interpreted as upward.
    // Distance from bottom: 3000 - 2200 - 600 = 200.
    await act(async () => {
      // Two events so direction detector sees "up" (newTop < lastTop)
      Object.defineProperty(handle.scroller, "scrollTop", { value: 2900, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
      Object.defineProperty(handle.scroller, "scrollTop", { value: 2200, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
    });
    expect(handle.pinned()).toBe(false);
  });

  it("clears hasNewContent when the user manually scrolls back near the bottom", async () => {
    // The other re-pin path: instead of clicking the button, the user just
    // scrolls back down. As long as they get within the 60px stick zone of
    // the *current* scrollHeight, both pin state and the new-content badge
    // should reset.
    const { handle } = renderHarness("session-1");
    setLayout(handle.scroller, { scrollHeight: 3000, clientHeight: 600 });
    await flushAsync();

    // Up + growth
    await act(async () => {
      Object.defineProperty(handle.scroller, "scrollTop", { value: 200, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
    });
    setLayout(handle.scroller, { scrollHeight: 6000, clientHeight: 600 });
    await act(async () => {
      const node = document.createElement("p");
      node.textContent = "more";
      handle.scroller.appendChild(node);
    });
    await flushAsync();
    expect(handle.hasNewContent()).toBe(true);

    // Manually scroll back into the pin zone (6000 - 5380 - 600 = 20, < 60).
    await act(async () => {
      Object.defineProperty(handle.scroller, "scrollTop", { value: 5380, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
    });
    expect(handle.pinned()).toBe(true);
    expect(handle.hasNewContent()).toBe(false);
  });

  // ── New regression: cosmetic re-renders must not light up the badge ────────

  it("does NOT mark hasNewContent when a mutation happens but scrollHeight is unchanged (shiki / re-render)", async () => {
    // Real failure mode in the running app: user scrolls up to read a long
    // assistant message. The message contains a fenced code block. Shiki
    // finishes async-highlighting → it swaps innerHTML of the <code>
    // element → MutationObserver fires → the OLD code lit the "↓ 新内容"
    // badge even though the rendered HEIGHT did not change and nothing
    // new actually arrived.
    //
    // Discriminator: was there real growth? We track lastSeenScrollHeight
    // at pin-loss and only flag hasNewContent when scrollHeight has grown
    // past that baseline.
    const { handle } = renderHarness("session-1");
    setLayout(handle.scroller, { scrollHeight: 3000, clientHeight: 600 });
    await flushAsync();

    // User scrolls up to read.
    await act(async () => {
      Object.defineProperty(handle.scroller, "scrollTop", { value: 500, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
    });
    expect(handle.pinned()).toBe(false);
    expect(handle.hasNewContent()).toBe(false);

    // Now simulate shiki rewriting an inner code node — same scrollHeight,
    // different DOM. (Replacing innerHTML triggers childList+characterData
    // mutations on the subtree.)
    const codeNode = document.createElement("pre");
    codeNode.innerHTML = "<code>before</code>";
    await act(async () => { handle.scroller.appendChild(codeNode); });
    // scrollHeight intentionally NOT bumped: this mutation does not add
    // real new content for the user.
    await act(async () => {
      codeNode.innerHTML = "<code class='hl'><span>after</span></code>";
    });
    await flushAsync();

    expect(handle.hasNewContent()).toBe(false);
    expect(handle.scroller.scrollTop).toBe(500); // still at user's position
  });

  it("DOES mark hasNewContent when scrollHeight actually grows while user is up", async () => {
    // Companion to the test above — make sure the discriminator doesn't
    // suppress the real signal.
    const { handle } = renderHarness("session-1");
    setLayout(handle.scroller, { scrollHeight: 3000, clientHeight: 600 });
    await flushAsync();

    await act(async () => {
      Object.defineProperty(handle.scroller, "scrollTop", { value: 500, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
    });
    expect(handle.hasNewContent()).toBe(false);

    // Real streaming growth — bumps scrollHeight past baseline.
    setLayout(handle.scroller, { scrollHeight: 6000, clientHeight: 600 });
    await act(async () => {
      const node = document.createElement("p");
      node.textContent = "fresh streaming";
      handle.scroller.appendChild(node);
    });
    await flushAsync();
    expect(handle.hasNewContent()).toBe(true);
  });

  it("does not treat a user disclosure expansion as new streamed content", async () => {
    const { handle, rerenderSignal } = renderSignalHarness("turn:10");
    setLayout(handle.scroller, { scrollHeight: 3000, clientHeight: 600 });
    await flushAsync();

    await act(async () => {
      Object.defineProperty(handle.scroller, "scrollTop", { value: 500, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
    });
    expect(handle.pinned()).toBe(false);

    // Expanding already-present evidence increases layout height, but the
    // actual conversation content signal is unchanged.
    setLayout(handle.scroller, { scrollHeight: 4200 });
    await act(async () => {
      rerenderSignal("turn:10");
      const details = document.createElement("div");
      details.textContent = "already received routine evidence";
      handle.scroller.appendChild(details);
    });
    await flushAsync();

    expect(handle.hasNewContent()).toBe(false);
    expect(handle.scroller.scrollTop).toBe(500);

    // A real token/tool update changes the signal and should still light the
    // badge when it produces additional height.
    setLayout(handle.scroller, { scrollHeight: 4400 });
    await act(async () => {
      rerenderSignal("turn:11");
    });
    await act(async () => {
      const token = document.createElement("p");
      token.textContent = "fresh streamed content";
      handle.scroller.appendChild(token);
    });
    await flushAsync();
    expect(handle.hasNewContent()).toBe(true);
  });

  it("re-pins and snaps when the user reaches the bottom they could SEE, even though streaming outran the 240px zone", async () => {
    // v1.49.0 field report: "我已经拉到最下边了,最新的信息还是不会自动滚".
    // The user drags the thumb to the bottom of what they can see; by the
    // time the scroll event is handled, streaming has already pushed the
    // real bottom hundreds of px further — distFromBottom exceeds even the
    // 240px catch-up zone, so the old logic never re-pins and the view
    // stays frozen. Reaching the LAST KNOWN bottom (from the previous
    // scroll event) must count as "user wants the tail": re-pin AND snap
    // to the true bottom immediately.
    const { handle } = renderHarness("session-1");
    setLayout(handle.scroller, { scrollHeight: 2000, clientHeight: 500 });
    await flushAsync();

    // User scrolls up — unpin. Last known bottom at this event: 1500.
    await act(async () => {
      Object.defineProperty(handle.scroller, "scrollTop", { value: 900, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
    });
    expect(handle.pinned()).toBe(false);

    // They drag all the way down to the bottom they could see (1500)…
    // …but the stream has already grown the content to 3000 when the
    // event is processed: distFromBottom = 3000 - 1500 - 500 = 1000.
    await act(async () => {
      setLayout(handle.scroller, { scrollHeight: 3000 });
      Object.defineProperty(handle.scroller, "scrollTop", { value: 1500, configurable: true, writable: true });
      handle.scroller.dispatchEvent(new Event("scroll"));
    });
    await flushAsync();

    expect(handle.pinned()).toBe(true);
    // Re-pin must include an immediate snap to the TRUE bottom — waiting
    // for the next mutation leaves the user parked mid-stream.
    expect(handle.scroller.scrollTop).toBe(3000);

    // And the follow must survive subsequent streaming.
    setLayout(handle.scroller, { scrollHeight: 4000 });
    await act(async () => {
      const node = document.createElement("div");
      node.textContent = "next token batch";
      handle.scroller.appendChild(node);
    });
    await flushAsync();
    expect(handle.scroller.scrollTop).toBe(4000);
  });

  it("echo of a clamped programmatic scroll is not mistaken for user scrolling", async () => {
    // Real browsers clamp `scrollTop = scrollHeight` writes to
    // scrollHeight - clientHeight; jsdom does not, so emulate the clamp.
    // The echo detector must record the CLAMPED position — recording the
    // raw written value makes every echo look like a user scroll.
    const { handle } = renderHarness("session-1");
    let top = 0;
    const clamp = () =>
      Math.max(0, handle.scroller.scrollHeight - handle.scroller.clientHeight);
    Object.defineProperty(handle.scroller, "scrollTop", {
      configurable: true,
      get: () => top,
      set: (v: number) => { top = Math.min(Math.max(0, v), clamp()); },
    });
    setLayout(handle.scroller, { scrollHeight: 2000, clientHeight: 500 });

    await act(async () => {
      const node = document.createElement("div");
      node.textContent = "grow";
      handle.scroller.appendChild(node);
    });
    await flushAsync();
    expect(handle.scroller.scrollTop).toBe(1500); // clamped snap

    // The browser fires the echo scroll event at the clamped position,
    // within the ignore window. It must be ignored — and in particular a
    // user up-scroll right after must still unpin cleanly.
    await act(async () => {
      handle.scroller.dispatchEvent(new Event("scroll"));
    });
    expect(handle.pinned()).toBe(true);

    await act(async () => {
      handle.scroller.scrollTop = 700;
      handle.scroller.dispatchEvent(new Event("scroll"));
    });
    expect(handle.pinned()).toBe(false);
  });

});
