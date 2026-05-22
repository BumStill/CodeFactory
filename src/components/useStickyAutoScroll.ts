// SPDX-License-Identifier: Apache-2.0
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

/**
 * Keep a scrollable element pinned to the bottom while content grows, but
 * back off the moment the user scrolls up. Switching conversations forces
 * a re-pin so the previous conversation's "scrolled-up" state does not
 * leak across.
 *
 * Why MutationObserver and not ResizeObserver: a sentinel `<div>` never
 * changes its own dimensions when content above it grows, so a
 * ResizeObserver on the sentinel never fires — that was the bug we
 * shipped in 0.3.7 and again in 0.3.9 before this hook was extracted
 * and unit-tested.
 *
 * Why a `conversationKey` argument: state inside this hook persists
 * across re-renders. Without an explicit boundary signal the hook can't
 * tell "new conversation just loaded — snap to bottom" from "same
 * conversation, user scrolled up — leave them alone."
 */
export function useStickyAutoScroll(conversationKey: string | null) {
  const scrollerRef = useRef<HTMLDivElement>(null);
  const [pinned, setPinned] = useState(true);
  const pinnedRef = useRef(true);
  pinnedRef.current = pinned;

  const prevKeyRef = useRef<string | null>(null);

  // User-scroll handler: maintain pin state from observed scroll position.
  useEffect(() => {
    const el = scrollerRef.current;
    if (!el) return;
    const onScroll = () => {
      const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
      if (nearBottom !== pinnedRef.current) setPinned(nearBottom);
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, []);

  // Content-growth observer: any DOM mutation inside the scroller triggers
  // a stick-to-bottom check.
  useLayoutEffect(() => {
    const scroller = scrollerRef.current;
    if (!scroller) return;

    const stickToBottom = () => {
      if (pinnedRef.current) {
        scroller.scrollTop = scroller.scrollHeight;
      }
    };

    const obs = new MutationObserver(stickToBottom);
    obs.observe(scroller, {
      childList: true,
      subtree: true,
      characterData: true,
    });

    // Initial paint: snap to bottom so reloaded conversations land at the
    // latest message instead of at the top.
    requestAnimationFrame(stickToBottom);

    return () => obs.disconnect();
  }, []);

  // Render-tick fallback for the MutationObserver.
  // Background: in production we shipped a bug where streaming content
  // stopped following scroll the moment the rendering switched from the
  // "Thinking…" placeholder to the prose-styled markdown subtree. The
  // observer's microtask sometimes fires BEFORE the new subtree's layout
  // is fully computed, so it reads a stale scrollHeight and we end up a
  // viewport short of the actual bottom. Re-running stickToBottom inside
  // a layout effect that depends on `scrollTick` (incremented by the
  // caller on every interesting state change — see MessageList) guarantees
  // one extra snap per render, after React has committed the new layout.
  useLayoutEffect(() => {
    const el = scrollerRef.current;
    if (!el) return;
    if (pinnedRef.current) {
      el.scrollTop = el.scrollHeight;
    }
  });

  // Session-switch effect: force re-pin and snap to bottom when the
  // conversation identity changes. Runs after new messages have been
  // committed so scrollHeight reflects them.
  useLayoutEffect(() => {
    if (prevKeyRef.current === conversationKey) return;
    prevKeyRef.current = conversationKey;
    const el = scrollerRef.current;
    if (!el) return;
    setPinned(true);
    pinnedRef.current = true;
    // Two RAFs: first for layout, second to catch any post-layout shiki
    // highlight that bumps heights by a hair.
    requestAnimationFrame(() => {
      el.scrollTop = el.scrollHeight;
      requestAnimationFrame(() => {
        el.scrollTop = el.scrollHeight;
      });
    });
  }, [conversationKey]);

  const jumpToBottom = useCallback(() => {
    const el = scrollerRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
    setPinned(true);
  }, []);

  return { scrollerRef, pinned, jumpToBottom };
}
