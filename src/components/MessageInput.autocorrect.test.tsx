import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { MessageInput } from "./MessageInput";

vi.mock("../lib/tauri", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ convertFileSrc: (path: string) => path }));

/**
 * M44 / CF-INP-R1.
 *
 * The stored message had turned `--3way` into `—3way`: macOS WebKit applies
 * "smart" text substitution inside an editable element unless the element
 * says otherwise. Those substitutions are gated by three attributes —
 * `autocorrect` (smart dashes / smart quotes / auto-correction),
 * `autocapitalize` and `spellcheck`. A coding tool's composer must disable
 * all three, otherwise the webview rewrites what the user typed before the
 * value ever reaches the send path.
 *
 * The DOM value is asserted, not React state: the substitution happens in the
 * engine that owns the element, below React.
 */
describe("composer disables platform text substitution", () => {
  it("renders autocorrect/autocapitalize off and spellcheck off", () => {
    render(
      <MessageInput onSend={vi.fn()} onCancel={() => {}} streaming={false} disabled={false} cwd="/tmp" />,
    );
    const composer = screen.getByRole("textbox");
    expect(composer.getAttribute("autocorrect")).toBe("off");
    expect(composer.getAttribute("autocapitalize")).toBe("off");
    expect(composer.getAttribute("spellcheck")).toBe("false");
  });
});
