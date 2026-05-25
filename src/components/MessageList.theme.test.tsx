// SPDX-License-Identifier: Apache-2.0
//
// Theme-readability regression tests for MessageList.
//
// jsdom doesn't compute CSS, so we can't measure actual rendered contrast
// here. What we CAN guarantee is that styling decisions which break light
// mode (e.g. unconditional `prose-invert`, hardcoded `text-amber-200` on
// inline code) don't sneak back in. Each test below corresponds to a real
// visual bug we shipped in v0.5.1 and want to prevent regressing.

import { describe, it, expect } from "vitest";
import { render } from "@testing-library/react";
import { MessageList } from "./MessageList";
import type { UIMessage } from "../stores/chatEvents";

const baseMsg = (over: Partial<UIMessage> = {}): UIMessage => ({
  id: "m1",
  role: "assistant",
  content: "Hello **world**, use `foo()` to start.",
  createdAt: Date.now(),
  ...over,
});

describe("MessageList theme readability", () => {

  it("never applies prose-invert unconditionally — only via dark: prefix", () => {
    // prose-invert forces a dark-mode color palette on rendered markdown
    // (white headings, light-gray body, etc.). On a light surface this
    // becomes effectively invisible — which is exactly what shipped in v0.5.1.
    //
    // The fix is to gate it behind Tailwind's `dark:` prefix, which our
    // tailwind.config already maps to `[data-theme="dark"]`. The test fails
    // if a future change resurrects the unconditional class.
    const { container } = render(
      <MessageList messages={[baseMsg()]} streaming={false} cwd={null} />,
    );
    const prose = container.querySelector(".prose");
    expect(prose, "expected a .prose markdown container").toBeTruthy();

    const cls = prose!.className;
    // Allowed:    "prose dark:prose-invert ..."
    // Forbidden:  "prose prose-invert ..."  (unconditional)
    expect(cls).toMatch(/\bdark:prose-invert\b/);
    expect(cls).not.toMatch(/(^|\s)prose-invert(\s|$)/);
  });

  it("inline code color is theme-aware (not hardcoded text-amber-200)", () => {
    // amber-200 (#fde68a) on a white background = ~1.3:1 contrast — invisible.
    // The correct pattern is `text-amber-700 dark:text-amber-200` (or any
    // light-mode-readable base color), which produces ~7.5:1 on white and
    // ~6:1 on dark surface.
    const { container } = render(
      <MessageList messages={[baseMsg()]} streaming={false} cwd={null} />,
    );
    const code = container.querySelector("code");
    expect(code, "expected an inline <code> element").toBeTruthy();

    const cls = code!.className;
    // Forbidden: unconditional text-amber-200 (was the v0.5.1 bug).
    expect(cls).not.toMatch(/(^|\s)text-amber-200(\s|$)/);
  });

});
