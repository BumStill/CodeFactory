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
      activeSession: null,
      draftSession: null,
      runtime: {},
      activeModel: "deepseek-v4",
      _unlisten: {},
      _unlistenSessionUpdated: {},
      _streamingMsgId: {},
    });
  });

  it("creates an in-memory draft without calling the backend", () => {
    const draft = useChatStore.getState().beginDraft();

    expect(draft).toEqual(expect.objectContaining({
      id: expect.any(String),
      cwd: null,
      anonymous: false,
      modelId: "deepseek-v4",
    }));
    expect(useChatStore.getState().activeSession).toBeNull();
    expect(useChatStore.getState().draftSession?.id).toBe(draft.id);
    expect(mocks.invoke).not.toHaveBeenCalled();
  });

  it("scoping a draft to a project keeps the SAME blank conversation", () => {
    // The bug this guards: picking a project used to be able to move the user
    // into that project's previous conversation. Scope is a property of the
    // draft — choosing one must never load history or swap the session.
    const draft = useChatStore.getState().beginDraft();

    useChatStore.getState().setDraftProject("/Users/x/project");

    const state = useChatStore.getState();
    expect(state.draftSession?.id).toBe(draft.id);
    expect(state.draftSession?.cwd).toBe("/Users/x/project");
    expect(state.activeSession).toBeNull();
    expect(mocks.invoke).not.toHaveBeenCalled();
  });

  it("opens a blank draft for a project that already has sessions", () => {
    const existing = { ...materialized, id: "old", cwd: "/Users/x/project", kind: "project" as const };
    useChatStore.setState({ sessions: [existing] });

    const draft = useChatStore.getState().beginDraft({ cwd: "/Users/x/project" });

    const state = useChatStore.getState();
    expect(draft.id).not.toBe(existing.id);
    expect(state.draftSession?.cwd).toBe("/Users/x/project");
    expect(state.activeSession).toBeNull();
    expect(mocks.invoke).not.toHaveBeenCalledWith("get_message_page", expect.anything());
  });

  it("materializes exactly once on first send, then streams through the real session", async () => {
    useChatStore.setState({ draftSession: {
      id: "draft-1",
      cwd: null,
      anonymous: false,
      modelId: "deepseek-v4",
      text: "",
    } });
    mocks.invoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "materialize_draft_session") {
        expect(args).toEqual({
          draftId: "draft-1",
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
    expect(useChatStore.getState().sessions[0]?.id).toBe("draft-1");
    expect(useChatStore.getState().runtime["draft-1"] ?? freshRuntime()).toEqual(
      expect.objectContaining({ streaming: true }),
    );
  });

  it("lands on a blank draft in the same project after deleting the open conversation", async () => {
    // The workspace renders whatever the store says is open, so deleting the
    // conversation you are IN must leave something behind — otherwise the shell
    // has nothing to show at all.
    const open = { ...materialized, id: "open", cwd: "/Users/x/project", kind: "project" as const };
    useChatStore.setState({ sessions: [open], activeSession: open, draftSession: null });

    await useChatStore.getState().deleteSession("open");

    const state = useChatStore.getState();
    expect(state.sessions).toEqual([]);
    expect(state.activeSession).toBeNull();
    expect(state.draftSession?.cwd).toBe("/Users/x/project");
  });

  it("leaves the open conversation alone when a different one is deleted", async () => {
    const open = { ...materialized, id: "open" };
    const other = { ...materialized, id: "other" };
    useChatStore.setState({ sessions: [open, other], activeSession: open, draftSession: null });

    await useChatStore.getState().deleteSession("other");

    const state = useChatStore.getState();
    expect(state.activeSession?.id).toBe("open");
    expect(state.draftSession).toBeNull();
  });

  it("falls back to a blank draft when a session id can no longer be opened", async () => {
    // A stale id used to leave the workspace pointing at a session that isn't
    // there: the rejection went unhandled and the PREVIOUS conversation stayed
    // on screen while every session-scoped feature addressed the dead id.
    const stale = { ...materialized, id: "gone" };
    useChatStore.setState({ sessions: [stale], activeSession: stale, draftSession: null });
    mocks.invoke.mockRejectedValueOnce(new Error("no such session: gone"));

    await expect(useChatStore.getState().selectSession("gone")).resolves.toBeUndefined();

    const state = useChatStore.getState();
    expect(state.activeSession).toBeNull();
    expect(state.draftSession).not.toBeNull();
    expect(state.draftSession?.cwd).toBeNull();
  });

  it("updates the in-memory draft model without calling session persistence", async () => {
    useChatStore.setState({ draftSession: {
      id: "draft-1",
      cwd: null,
      anonymous: false,
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
      cwd: "/Users/x/project",
      anonymous: false,
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
