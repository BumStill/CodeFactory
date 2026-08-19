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
export function useStickyAutoScroll(
  conversationKey: string | null,
  contentSignal?: unknown,
) {
  const scrollerRef = useRef<HTMLDivElement>(null);
  const [scrollerMounted, setScrollerMounted] = useState(false);
  const [pinned, setPinned] = useState(true);
  const pinnedRef = useRef(true);
  pinnedRef.current = pinned;

  // True iff content has grown since the user scrolled away from the
  // bottom. Drives the "↓ New content" pulse on the floating jump button
  // so the user can tell when there's something fresh to see vs just
  // being above the conversation tail.
  const [hasNewContent, setHasNewContent] = useState(false);
  const hasNewContentRef = useRef(false);
  hasNewContentRef.current = hasNewContent;

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

  // Last user scrollTop so we can detect scroll direction. Streaming
  // produces a "moving bottom" — when the user scrolls *toward* the tail
  // we use a wider re-pin threshold so they can actually catch up; when
  // they scroll *away* (up), a tight threshold keeps them detached the
  // moment they leave the stick zone.
  const lastUserScrollTop = useRef(0);

  // scrollHeight observed at the moment the user lost pin. Only growth
  // PAST this baseline counts as "new content" — re-renders that mutate
  // the DOM without growing height (shiki finishing async highlight,
  // React swapping attributes on existing nodes, typing-dot animation)
  // should NOT light up the "new content" badge. See the bug-reproducer
  // test in useStickyAutoScroll.test.tsx for the exact scenario.
  const newContentBaseline = useRef(0);
  // Layout can grow because the user expanded evidence that was already in
  // the conversation. That is not new streamed content. When the caller can
  // provide a semantic content signal, require it to change after pin loss
  // before lighting the badge; callers without a signal retain the legacy
  // height-only behavior.
  const contentSignalRef = useRef(contentSignal);
  contentSignalRef.current = contentSignal;
  const pinLossContentSignal = useRef(contentSignal);

  // Bottom position captured when pinning is lost. It remains stable while
  // the user drags back toward the tail; streaming must not move the target
  // between scroll events or the thumb can chase forever without re-pinning.
  const pinLossBottom = useRef(0);

  const programmaticScrollTo = useCallback((y: number) => {
    const el = scrollerRef.current;
    if (!el) return;
    ignoreScrollUntil.current = Date.now() + 80;
    el.scrollTop = y;
    // Record the CLAMPED position the browser actually applied — recording
    // the raw value (often scrollHeight, one clientHeight past the max)
    // would make every echo event look like a user scroll.
    lastSetScrollTop.current = el.scrollTop;
    // The next non-echo scroll event must measure direction from the position
    // the user actually saw after our pin. Leaving this at its initial zero
    // makes a first upward wheel gesture look stationary/downward and applies
    // the broad catch-up threshold, which can silently re-pin short overflows.
    lastUserScrollTop.current = el.scrollTop;
  }, []);

  const prepareForPrepend = useCallback(() => {
    const element = scrollerRef.current;
    if (!element) return null;
    const previousHeight = element.scrollHeight;
    const previousTop = element.scrollTop;

    // Prepending is programmatic history navigation, not new tail content.
    // Detach before React commits the older rows so MutationObserver cannot
    // snap to bottom or light the "new content" badge for those rows.
    pinnedRef.current = false;
    setPinned(false);
    newContentBaseline.current = Number.POSITIVE_INFINITY;

    return {
      element,
      restore: () => {
        if (scrollerRef.current !== element) return;
        const nextTop =
          previousTop + Math.max(0, element.scrollHeight - previousHeight);
        programmaticScrollTo(nextTop);
        lastUserScrollTop.current = element.scrollTop;
        newContentBaseline.current = element.scrollHeight;
        pinLossBottom.current = Math.max(
          0,
          element.scrollHeight - element.clientHeight,
        );
      },
    };
  }, [programmaticScrollTo]);

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

      const newTop = el.scrollTop;
      // Direction. "still" treated as "down" because the user is
      // probably done scrolling; we want to re-pin generously.
      const scrollingUp = newTop < lastUserScrollTop.current - 1;
      lastUserScrollTop.current = newTop;

      const distFromBottom = el.scrollHeight - newTop - el.clientHeight;
      // The whole point of two thresholds: streaming pushes scrollHeight
      // faster than a wheel can keep up, so chasing the tail with a 60px
      // window is impossible. When the user is scrolling DOWN they get a
      // 240px catch-up zone; up-scroll keeps the tight 60px so leaving
      // the stick zone is definitive.
      const threshold = scrollingUp ? 60 : 240;
      // A fast stream can outrun even the 240px zone between the drag and
      // this handler. Reaching the bottom the user could SEE (the bottom
      // as of the previous event) is an unambiguous "give me the tail" —
      // count it regardless of how far the live bottom has moved since.
      const reachedSeenBottom =
        !scrollingUp && !pinnedRef.current && newTop >= pinLossBottom.current - 60;
      const nearBottom = distFromBottom < threshold || reachedSeenBottom;

      if (nearBottom !== pinnedRef.current) {
        // Update ref synchronously so any MutationObserver hit in the same
        // microtask sees the new value (without this they could race and
        // re-pin before React commits the state).
        pinnedRef.current = nearBottom;
        setPinned(nearBottom);
        if (nearBottom && hasNewContentRef.current) {
          // Scrolled back into pin zone — user has caught up to the tail,
          // the "new content" indicator's purpose is served.
          hasNewContentRef.current = false;
          setHasNewContent(false);
        }
        if (nearBottom && distFromBottom > 2) {
          // Re-pinned away from the live bottom (streaming outran the
          // drag): snap immediately instead of waiting for the next
          // mutation, or the user sits parked mid-stream.
          programmaticScrollTo(el.scrollHeight);
        }
        if (!nearBottom) {
          // Capture both growth and the bottom the user could see at the exact
          // moment they detached. Keep this bottom stable until they re-pin.
          newContentBaseline.current = el.scrollHeight;
          pinLossContentSignal.current = contentSignalRef.current;
          pinLossBottom.current = Math.max(0, el.scrollHeight - el.clientHeight);
        }
      }
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, [programmaticScrollTo, scrollerMounted]);

  // Content-growth observer: every DOM mutation inside the scroller is a
  // potential reason to re-stick. We also queue a RAF retick to catch
  // height changes that arrive in a subsequent layout pass (shiki async
  // highlighting, markdown subtree replacement, etc.).
  useLayoutEffect(() => {
    const scroller = scrollerRef.current;
    if (!scroller) return;

    const onMutation = () => {
      // Discriminator: only flag "new content" when scrollHeight has
      // actually grown past the baseline captured at pin-loss. Pure
      // DOM-mutation events (shiki, React re-renders) don't qualify.
      if (
        !pinnedRef.current &&
        !hasNewContentRef.current &&
        (contentSignalRef.current === undefined ||
          contentSignalRef.current !== pinLossContentSignal.current) &&
        scroller.scrollHeight > newContentBaseline.current
      ) {
        hasNewContentRef.current = true;
        setHasNewContent(true);
      }
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
  }, [scrollerMounted, stickToBottomIfPinned]);

  // Track when the scroller DOM node appears or disappears so observers
  // are reattached (e.g. WelcomeScreen → first message transition).
  useLayoutEffect(() => {
    setScrollerMounted(!!scrollerRef.current);
  });

  // Session-switch reset: force re-pin and snap to bottom when the
  // conversation identity changes. The double-RAF mirrors the late-layout
  // logic above for shiki / markdown.
  useLayoutEffect(() => {
    if (prevKeyRef.current === conversationKey) return;
    prevKeyRef.current = conversationKey;
    if (!scrollerRef.current) return;
    pinnedRef.current = true;
    newContentBaseline.current = scrollerRef.current.scrollHeight;
    pinLossContentSignal.current = contentSignalRef.current;
    pinLossBottom.current = Math.max(
      0,
      scrollerRef.current.scrollHeight - scrollerRef.current.clientHeight,
    );
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
    if (hasNewContentRef.current) {
      hasNewContentRef.current = false;
      setHasNewContent(false);
    }
    programmaticScrollTo(el.scrollHeight);
  }, [programmaticScrollTo]);

  // Reset hasNewContent when the conversation changes — the badge would
  // otherwise persist across session switches and lie about what's fresh.
  useLayoutEffect(() => {
    if (hasNewContentRef.current) {
      hasNewContentRef.current = false;
      setHasNewContent(false);
    }
  }, [conversationKey]);

  return {
    scrollerRef,
    pinned,
    hasNewContent,
    jumpToBottom,
    prepareForPrepend,
  };
}
