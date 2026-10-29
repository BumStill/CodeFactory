// SPDX-License-Identifier: Apache-2.0
import { beforeEach, expect, it, vi } from "vitest";
const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("./tauri", () => ({ invoke: mocks.invoke, onStream: vi.fn(async () => () => {}), onSessionUpdated: vi.fn(async () => () => {}), sendMessageAnonymous: vi.fn() }));
vi.mock("@tauri-apps/api/app", () => ({ setTheme: vi.fn() }));
import { useChatStore, freshRuntime } from "../stores/chat";
import { dispatchSend } from "./dispatchActions";
import type { Session } from "./tauri";

beforeEach(() => {
  mocks.invoke.mockReset();
  const session = { id: "synthetic-m200", kind: "project", model_id: "synthetic", cwd: "/synthetic" } as Session;
  useChatStore.setState({ sessions: [session], activeSession: session, runtime: { [session.id]: freshRuntime() } });
});

it("M200 R2 persisted root with a model wake returns queued success after startup timeout", async () => {
  mocks.invoke.mockImplementation(async (command: string, args: Record<string, unknown>) => {
    if (command === "send_message") return new Promise(() => {});
    if (command === "dispatch_delivery_receipt") return { session_id: "synthetic-m200", root_turn_id: args.rootTurnId, status: "queued_waiting_model", next_at: 2000000000, turn_exists: true };
  });
  const result = await dispatchSend({ sessionId: "synthetic-m200", message: "synthetic task", mode: "steer" }, { timeoutMs: 20 });
  expect(result).toMatchObject({ session_id: "synthetic-m200", status: "queued_waiting_model", next_at: 2000000000 });
  const root = useChatStore.getState().runtime["synthetic-m200"].messages.find(message => message.role === "user")!.id;
  expect(mocks.invoke).toHaveBeenCalledWith("dispatch_delivery_receipt", { sessionId: "synthetic-m200", rootTurnId: root, message: "synthetic task" });
});

it("M200 R2 optimistic bubble without durable root still fails", async () => {
  mocks.invoke.mockImplementation(async (command: string) => {
    if (command === "send_message") return new Promise(() => {});
    if (command === "dispatch_delivery_receipt") return null;
  });
  await expect(dispatchSend({ sessionId: "synthetic-m200", message: "not persisted", mode: "steer" }, { timeoutMs: 20 })).rejects.toMatchObject({ code: "delivery_failed" });
});
