// SPDX-License-Identifier: Apache-2.0
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
const mock = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("../lib/tauri", () => ({ invoke: mock.invoke }));
vi.mock("../stores/chat", () => ({ useChatStore: (select: (state: unknown) => unknown) => select({ activeSession: { id: "synthetic-session", kind: "project" } }) }));
import { ReleaseUrgencyPicker } from "./ReleaseUrgencyPicker";
beforeEach(() => { mock.invoke.mockReset(); mock.invoke.mockResolvedValue("immediate"); });
it("R3 loads persisted urgency and saves a user hold", async () => {
  render(<ReleaseUrgencyPicker />);
  const select = screen.getByRole("combobox", { name: "发版紧急度" });
  await waitFor(() => expect(select).toHaveValue("immediate"));
  fireEvent.change(select, { target: { value: "hold" } });
  await waitFor(() => expect(mock.invoke).toHaveBeenCalledWith("update_session_release_urgency", { sessionId: "synthetic-session", urgency: "hold" }));
  await waitFor(() => expect(select).toHaveValue("hold"));
});
it("R3 reports persistence failure without claiming a changed policy", async () => {
  render(<ReleaseUrgencyPicker />);
  await waitFor(() => expect(screen.getByRole("combobox")).toHaveValue("immediate"));
  mock.invoke.mockRejectedValueOnce(new Error("synthetic save failure"));
  fireEvent.change(screen.getByRole("combobox"), { target: { value: "default" } });
  expect(await screen.findByRole("alert")).toHaveTextContent("synthetic save failure");
  expect(screen.getByRole("combobox")).toHaveValue("immediate");
});
