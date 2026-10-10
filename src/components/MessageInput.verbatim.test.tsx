import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MessageInput } from "./MessageInput";

vi.mock("../lib/tauri", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ convertFileSrc: (path: string) => path }));

describe("verbatim composer input", () => {
  it.each(["--flag --ci", '"quoted" \'single\'', "... Chinese 中文", "git apply --3way --index"])("sends typed text verbatim: %s", async (text) => {
    const onSend = vi.fn();
    render(<MessageInput onSend={onSend} onCancel={() => {}} streaming={false} disabled={false} cwd="/tmp" />);
    const input = screen.getByRole("textbox");
    fireEvent.change(input, { target: { value: text } });
    fireEvent.click(screen.getByLabelText("发送"));
    await waitFor(() => expect(onSend).toHaveBeenCalledWith(text));
  });
});
