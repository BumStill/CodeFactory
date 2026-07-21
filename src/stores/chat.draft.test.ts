// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { freshRuntime, useChatStore } from "./chat";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  onStream: vi.fn(),
  onSessionUpdated: vi.fn(),
}));

vi.mock("../lib/tauri", () => ({
  invoke: mocks.invoke,
  onStream: mocks.onStream,
  onSessionUpdated: mocks.onSessionUpdated,
  sendMessageAnonymous: vi.fn(),
}));

const materialized = {
  id: "draft-1",
  title: "第一条真实消息",
  cwd: "/tmp/quick/draft-1",
  model_id: "deepseek-v4",
  created_at: 1,
  updated_at: 1,
  total_input_tokens: 0,
  total_output_tokens: 0,
  kind: "quick" as const,
};

describe("lazy draft session", () => {
  beforeEach(() => {
    Object.values(mocks).forEach((mock) => mock.mockReset());
    mocks.onStream.mockResolvedValue(() => {});
    mocks.onSessionUpdated.mockResolvedValue(() => {});
    useChatStore.setState({
      sessions: [],
      quickSessions: [],
      activeSession: null,
      draftSession: null,
      runtime: {},
      activeModel: "deepseek-v4",
      _unlisten: {},
      _unlistenSessionUpdated: {},
      _streamingMsgId: {},
    });
  });

  it("creates an in-memory quick draft without calling the backend", () => {
    const draft = useChatStore.getState().beginQuickDraft();

    expect(draft).toEqual(expect.objectContaining({
      id: expect.any(String),
      mode: "quick",
      cwd: null,
      modelId: "deepseek-v4",
    }));
    expect(useChatStore.getState().activeSession).toBeNull();
    expect(useChatStore.getState().draftSession?.id).toBe(draft.id);
    expect(mocks.invoke).not.toHaveBeenCalled();
  });

  it("materializes exactly once on first send, then streams through the real session", async () => {
    useChatStore.setState({ draftSession: {
      id: "draft-1",
      mode: "quick",
      cwd: null,
      modelId: "deepseek-v4",
      text: "",
    } });
    mocks.invoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "materialize_draft_session") {
        expect(args).toEqual({
          draftId: "draft-1",
          mode: "quick",
          cwd: null,
          modelId: "deepseek-v4",
          firstMessage: "第一条真实消息",
        });
        return Promise.resolve(materialized);
      }
      if (cmd === "send_message") return Promise.resolve(undefined);
      return Promise.resolve(undefined);
    });

    await Promise.all([
      useChatStore.getState().sendOrQueue("第一条真实消息"),
      useChatStore.getState().sendOrQueue("第一条真实消息"),
    ]);

    expect(mocks.invoke.mock.calls.filter(([cmd]) => cmd === "materialize_draft_session")).toHaveLength(1);
    expect(mocks.invoke).toHaveBeenCalledWith("send_message", {
      sessionId: "draft-1",
      content: "第一条真实消息",
      userMessagePersisted: true,
    });
    expect(useChatStore.getState().draftSession).toBeNull();
    expect(useChatStore.getState().activeSession?.id).toBe("draft-1");
    expect(useChatStore.getState().quickSessions[0]?.id).toBe("draft-1");
    expect(useChatStore.getState().runtime["draft-1"] ?? freshRuntime()).toEqual(
      expect.objectContaining({ streaming: true }),
    );
  });

  it("updates the in-memory draft model without calling session persistence", async () => {
    useChatStore.setState({ draftSession: {
      id: "draft-1",
      mode: "quick",
      cwd: null,
      modelId: "old-model",
      text: "",
    } });

    await useChatStore.getState().updateActiveSessionModel("new-model");

    expect(useChatStore.getState().activeModel).toBe("new-model");
    expect(useChatStore.getState().draftSession?.modelId).toBe("new-model");
    expect(mocks.invoke).not.toHaveBeenCalledWith("update_session_model", expect.anything());
  });

  it("keeps the draft and its text when materialization fails", async () => {
    useChatStore.setState({ draftSession: {
      id: "draft-1",
      mode: "project",
      cwd: "/Users/x/project",
      modelId: "deepseek-v4",
      text: "不能丢失的需求",
    } });
    mocks.invoke.mockRejectedValueOnce(new Error("database unavailable"));

    const result = await useChatStore.getState().sendOrQueue("不能丢失的需求");

    expect(result).toBe("failed");
    expect(useChatStore.getState().draftSession).toEqual(expect.objectContaining({
      id: "draft-1",
      cwd: "/Users/x/project",
      text: "不能丢失的需求",
    }));
    expect(useChatStore.getState().activeSession).toBeNull();
    expect(mocks.onStream).not.toHaveBeenCalled();
  });
});
