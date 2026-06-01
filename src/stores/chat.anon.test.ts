// SPDX-License-Identifier: Apache-2.0
//
// Anonymous-chat store flow. Verifies the privacy contract at the store seam:
// anonymous turns go to `send_message_anonymous` (NOT the persisted
// `send_message`), the frontend replays its own history, exiting discards
// everything, and re-selecting the ephemeral session never hits the DB.
// State now lives in per-session runtime buckets (keyed by session id).

import { describe, it, expect, vi, beforeEach } from "vitest";
import { useChatStore, activeRuntime, freshRuntime, type SessionRuntime } from "./chat";

const invokeMock = vi.hoisted(() => vi.fn());
const sendAnonMock = vi.hoisted(() => vi.fn());
vi.mock("../lib/tauri", () => ({
  invoke: invokeMock,
  onStream: vi.fn(async () => () => {}),
  onSessionUpdated: vi.fn(async () => () => {}),
  sendMessageAnonymous: sendAnonMock,
}));

/** Merge a patch into a specific session's runtime bucket. */
function seedRuntime(id: string, patch: Partial<SessionRuntime>) {
  useChatStore.setState((s) => ({
    runtime: { ...s.runtime, [id]: { ...(s.runtime[id] ?? freshRuntime()), ...patch } },
  }));
}

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
  sendAnonMock.mockReset();
  sendAnonMock.mockResolvedValue(undefined);
  useChatStore.setState({
    sessions: [],
    quickSessions: [],
    activeSession: null,
    runtime: {},
    activeModel: "anthropic/claude-opus-4-7",
    _unlisten: {},
    _unlistenSessionUpdated: {},
    _streamingMsgId: {},
  });
});

describe("anonymous chat store flow", () => {
  it("startAnonymousSession creates a blank in-memory anonymous session", () => {
    const s = useChatStore.getState().startAnonymousSession();
    expect(s.kind).toBe("anonymous");
    const st = useChatStore.getState();
    expect(st.activeSession?.id).toBe(s.id);
    expect(st.activeSession?.kind).toBe("anonymous");
    expect(activeRuntime(st).messages).toEqual([]);
  });

  it("routes anonymous turns to send_message_anonymous with replayed history", async () => {
    const s = useChatStore.getState().startAnonymousSession();
    seedRuntime(s.id, {
      messages: [
        { id: "u1", role: "user", content: "hi", createdAt: 0 },
        { id: "a1", role: "assistant", content: "hello", createdAt: 0 },
      ],
    });

    await useChatStore.getState().sendMessage("how are you");

    expect(sendAnonMock).toHaveBeenCalledTimes(1);
    const [sid, content, history, cwd, model] = sendAnonMock.mock.calls[0];
    expect(sid).toBe(s.id);
    expect(content).toBe("how are you");
    expect(history).toEqual([
      { role: "user", content: "hi" },
      { role: "assistant", content: "hello" },
    ]);
    expect(cwd).toBe(""); // backend resolves the scratch dir
    expect(model).toBe("anthropic/claude-opus-4-7");
    // The persisted path must never run for an anonymous session.
    expect(invokeMock).not.toHaveBeenCalledWith("send_message", expect.anything());
  });

  it("non-anonymous sendMessage still uses the persisted send_message path", async () => {
    useChatStore.setState({
      activeSession: {
        id: "p1", title: "t", cwd: "/proj", model_id: "m", created_at: 0,
        updated_at: 0, total_input_tokens: 0, total_output_tokens: 0, kind: "project",
      } as never,
      runtime: { p1: freshRuntime() },
    });

    await useChatStore.getState().sendMessage("hello");

    expect(invokeMock).toHaveBeenCalledWith(
      "send_message",
      expect.objectContaining({ content: "hello" }),
    );
    expect(sendAnonMock).not.toHaveBeenCalled();
  });

  it("exitAnonymous discards the in-memory session and its history", () => {
    const s = useChatStore.getState().startAnonymousSession();
    seedRuntime(s.id, {
      messages: [{ id: "x", role: "user", content: "secret", createdAt: 0 }],
    });

    useChatStore.getState().exitAnonymous();

    const st = useChatStore.getState();
    expect(st.activeSession).toBeNull();
    expect(st.runtime[s.id]).toBeUndefined();
    expect(activeRuntime(st).messages).toEqual([]);
  });

  it("selectSession never hits the DB for the active anonymous session", async () => {
    const s = useChatStore.getState().startAnonymousSession();
    await useChatStore.getState().selectSession(s.id);
    expect(invokeMock).not.toHaveBeenCalledWith("get_session", expect.anything());
    expect(invokeMock).not.toHaveBeenCalledWith("get_messages", expect.anything());
    expect(useChatStore.getState().activeSession?.id).toBe(s.id);
  });
});
