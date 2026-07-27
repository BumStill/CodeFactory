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

/** Anonymity is a draft switch now: begin an anonymous draft and let the first
 *  send turn it into the in-memory session these tests exercise. */
function startAnonymous() {
  const draft = useChatStore.getState().beginDraft({ anonymous: true });
  useChatStore.setState({
    draftSession: null,
    activeSession: {
      id: draft.id,
      title: "匿名会话",
      cwd: "",
      model_id: draft.modelId,
      created_at: 0,
      updated_at: 0,
      total_input_tokens: 0,
      total_output_tokens: 0,
      kind: "anonymous",
    },
    runtime: { ...useChatStore.getState().runtime, [draft.id]: freshRuntime() },
  });
  return useChatStore.getState().activeSession!;
}

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
    activeSession: null,
    draftSession: null,
    runtime: {},
    activeModel: "anthropic/claude-opus-4-7",
    _unlisten: {},
    _unlistenSessionUpdated: {},
    _streamingMsgId: {},
  });
});

describe("anonymous chat store flow", () => {
  it("uses the backend-resolved model immediately after creating a project session", async () => {
    invokeMock.mockResolvedValueOnce({
      id: "p-resolved",
      title: "project",
      cwd: "/tmp/project",
      model_id: "deepseek-v4-pro",
      created_at: 1,
      updated_at: 1,
      total_input_tokens: 0,
      total_output_tokens: 0,
      kind: "project",
    });

    await useChatStore.getState().createSession(
      "/tmp/project",
      "anthropic/claude-opus-4-7",
    );

    expect(useChatStore.getState().activeModel).toBe("deepseek-v4-pro");
  });

  it("an anonymous draft becomes an in-memory session on first send, never a row", async () => {
    const draft = useChatStore.getState().beginDraft({ anonymous: true });

    await useChatStore.getState().sendOrQueue("secret question");

    const st = useChatStore.getState();
    expect(st.activeSession?.id).toBe(draft.id);
    expect(st.activeSession?.kind).toBe("anonymous");
    expect(st.draftSession).toBeNull();
    expect(invokeMock).not.toHaveBeenCalledWith(
      "materialize_draft_session",
      expect.anything(),
    );
    expect(sendAnonMock).toHaveBeenCalledTimes(1);
  });

  it("an anonymous draft can still be scoped to a project directory", async () => {
    useChatStore.getState().beginDraft({ cwd: "/proj", anonymous: true });

    await useChatStore.getState().sendOrQueue("look at this repo");

    const [, , , cwd] = sendAnonMock.mock.calls[0];
    expect(cwd).toBe("/proj");
    expect(useChatStore.getState().sessions).toEqual([]);
  });

  it("routes anonymous turns to send_message_anonymous with replayed history", async () => {
    const s = startAnonymous();
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
    const s = startAnonymous();
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
    const s = startAnonymous();
    await useChatStore.getState().selectSession(s.id);
    expect(invokeMock).not.toHaveBeenCalledWith("get_session", expect.anything());
    expect(invokeMock).not.toHaveBeenCalledWith("get_messages", expect.anything());
    expect(useChatStore.getState().activeSession?.id).toBe(s.id);
  });
});
