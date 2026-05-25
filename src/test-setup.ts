// SPDX-License-Identifier: Apache-2.0
import "@testing-library/jest-dom/vitest";
import { afterEach } from "vitest";
import { cleanup } from "@testing-library/react";

// Vitest doesn't auto-unmount React Testing Library renders between tests
// the way Jest does. Without this, every test leaves its mounted tree in
// document.body and the next test's renderHarness picks up a stale handle
// from the previous Harness's onMount — symptoms: random tests fail with
// scrollTop=0 because they're inspecting a different scroller.
afterEach(() => {
  cleanup();
});

// DOM-environment-only patches. Skip in node-env tests (e.g. the file-system
// audit suites) where HTMLElement isn't defined and these references throw
// at module-load time before any test runs.
if (typeof HTMLElement !== "undefined") {
  // jsdom doesn't implement Element.scrollTo and leaves scrollTop a no-op
  // writer. We need both to work for the stick-to-bottom hook tests —
  // without them the observer fires but setting scrollTop does nothing
  // observable. Patch globally so every test gets a real
  // scrollHeight/scrollTop pair.
  Object.defineProperty(HTMLElement.prototype, "scrollHeight", {
    configurable: true,
    get: function () {
      return (this as HTMLElement & { _scrollHeight?: number })._scrollHeight ?? 0;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get: function () {
      return (this as HTMLElement & { _clientHeight?: number })._clientHeight ?? 0;
    },
  });

  // Force RAF onto the macrotask queue so `await flushAsync()` (which awaits
  // a handful of setTimeout(0)s) actually drains them. jsdom ships its own
  // RAF implementation timed to a virtual frame clock that doesn't advance
  // in vitest's environment — meaning RAF callbacks queued from useLayoutEffect
  // never fire, and the hook's initial-snap-to-bottom never runs.
  // We unconditionally override (not guard on "if undefined") for that reason.
  globalThis.requestAnimationFrame = (cb: FrameRequestCallback): number => {
    return setTimeout(() => cb(performance.now()), 0) as unknown as number;
  };
  globalThis.cancelAnimationFrame = (id: number) => clearTimeout(id);
}
