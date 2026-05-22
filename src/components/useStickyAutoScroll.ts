// SPDX-License-Identifier: Apache-2.0
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

/**
 * Keep a scrollable element pinned to the bottom while content grows, but
 * back off the moment the user scrolls up. Switching conversations forces
 * a re-pin so the previous conversation's "scrolled-up" state does not
 * leak across.
 *
 * Two important pitfalls we had to solve:
 *
 * 1. **Telling user scroll from our own scroll**. Every time the auto-stick
 *    sets `scrollTop`, the browser fires a `scroll` event. Without a guard,
 *    that handler reads `scrollTop ≈ scrollHeight`, decides "near bottom →
 *    pinned=true", and the user can never escape — every wheel-up is
 *    immediately countered by the next MutationObserver tick.
 *
 *    Fix: stamp `ignoreScrollUntil` ~80ms in the future before any
 *    programmatic write. Handlers within that window are treated as ours
 *    and skip the state flip.
 *
 * 2. **Late-arriving layout**. When the streaming UI swaps from the
 *    "Thinking…" placeholder to the prose-rendered markdown subtree, the
 *    MutationObserver microtask sometimes fires before the new subtree's
 *    height is computed — `scroller.scrollHeight` reads short by one
 *    viewport, and we end up not quite at the bottom.
 *
 *    Fix: after every MutationObserver hit, also schedule a RAF retick.
 *    Belt-and-suspenders pattern; the RAF runs after the browser's next
 *    layout pass so any height bumps from shiki / markdown rendering are
 *    captured. NO unconditional `useLayoutEffect` render-tick fallback —
 *    that earlier attempt fired on every unrelated re-render in the tree
 *    and stomped on user scroll (the bug this comment fixes).
 *
 * Why MutationObserver and not ResizeObserver: a 0-height sentinel never
 * changes its own dimensions when content above it grows, so a
 * ResizeObserver on the sentinel never fires.
 *
 * Why `conversationKey`: opening a previous session must land at the
 * latest message, not at wherever the previous session was scrolled.
 * The hook needs an explicit signal that "this is a new viewport" —
 * pin state shouldn't leak across.
 */
export function useStickyAutoScroll(conversationKey: string | null) {
  const scrollerRef = useRef<HTMLDivElement>(null);
  const [pinned, setPinned] = useState(true);
  const pinnedRef = useRef(true);
  pinnedRef.current = pinned;

  const prevKeyRef = useRef<string | null>(null);

  // Programmatic-scroll guard. We need to ignore the scroll event the
  // browser fires in response to our OWN `scrollTop = X` writes — if we
  // didn't, every auto-stick would look like the user scrolled to the
  // bottom and re-pin, making it impossible to escape.
  //
  // Two complementary signals:
  //   - `lastSetScrollTop`: the value we last wrote. If onScroll sees the
  //     element at that exact value, the event is our echo.
  //   - `ignoreScrollUntil`: short ms window after a write, to also
  //     catch echo events that arrive after we've already moved on (e.g.
  //     scroll inertia rounding to a near-but-not-equal value).
  const lastSetScrollTop = useRef(-1);
  const ignoreScrollUntil = useRef(0);

  const programmaticScrollTo = useCallback((y: number) => {
    const el = scrollerRef.current;
    if (!el) return;
    lastSetScrollTop.current = y;
    ignoreScrollUntil.current = Date.now() + 80;
    el.scrollTop = y;
  }, []);

  const stickToBottomIfPinned = useCallback(() => {
    const el = scrollerRef.current;
    if (!el || !pinnedRef.current) return;
    programmaticScrollTo(el.scrollHeight);
  }, [programmaticScrollTo]);

  // User-scroll handler: detects whether the user has navigated away from
  // the bottom (more than 60px up) and updates pin state.
  useEffect(() => {
    const el = scrollerRef.current;
    if (!el) return;
    const onScroll = () => {
      // Echo detection: an event is "ours" only when BOTH the position
      // matches what we wrote AND we're inside the small post-write
      // window. A position difference of >2px is definitely the user
      // (they wheeled somewhere new), regardless of timing.
      const positionMatches = Math.abs(el.scrollTop - lastSetScrollTop.current) < 2;
      const insideWindow = Date.now() < ignoreScrollUntil.current;
      if (positionMatches && insideWindow) return;

      const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
      if (nearBottom !== pinnedRef.current) {
        // Update ref synchronously so any MutationObserver hit in the same
        // microtask sees the new value (without this they could race and
        // re-pin before React commits the state).
        pinnedRef.current = nearBottom;
        setPinned(nearBottom);
      }
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, []);

  // Content-growth observer: every DOM mutation inside the scroller is a
  // potential reason to re-stick. We also queue a RAF retick to catch
  // height changes that arrive in a subsequent layout pass (shiki async
  // highlighting, markdown subtree replacement, etc.).
  useLayoutEffect(() => {
    const scroller = scrollerRef.current;
    if (!scroller) return;

    const onMutation = () => {
      stickToBottomIfPinned();
      requestAnimationFrame(stickToBottomIfPinned);
    };

    const obs = new MutationObserver(onMutation);
    obs.observe(scroller, {
      childList: true,
      subtree: true,
      characterData: true,
    });

    // Initial paint snap.
    requestAnimationFrame(stickToBottomIfPinned);

    return () => obs.disconnect();
  }, [stickToBottomIfPinned]);

  // Session-switch reset: force re-pin and snap to bottom when the
  // conversation identity changes. The double-RAF mirrors the late-layout
  // logic above for shiki / markdown.
  useLayoutEffect(() => {
    if (prevKeyRef.current === conversationKey) return;
    prevKeyRef.current = conversationKey;
    if (!scrollerRef.current) return;
    pinnedRef.current = true;
    setPinned(true);
    requestAnimationFrame(() => {
      stickToBottomIfPinned();
      requestAnimationFrame(stickToBottomIfPinned);
    });
  }, [conversationKey, stickToBottomIfPinned]);

  const jumpToBottom = useCallback(() => {
    const el = scrollerRef.current;
    if (!el) return;
    pinnedRef.current = true;
    setPinned(true);
    programmaticScrollTo(el.scrollHeight);
  }, [programmaticScrollTo]);

  return { scrollerRef, pinned, jumpToBottom };
}
