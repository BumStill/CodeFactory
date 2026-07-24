// SPDX-License-Identifier: Apache-2.0

import { beforeEach, describe, expect, it, vi } from "vitest";
import { dbMessagesToUI, freshRuntime, useChatStore } from "./chat";
import type { Message, MessagePage, Session } from "../lib/tauri";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("../lib/tauri", () => ({
  invoke: invokeMock,
  onStream: vi.fn(async () => () => {}),
  onSessionUpdated: vi.fn(async () => () => {}),
  sendMessageAnonymous: vi.fn(async () => {}),
}));

const session: Session = {
  id: "long",
  title: "long",
  cwd: "/project",
  model_id: "model",
  created_at: 1,
  updated_at: 1,
  total_input_tokens: 0,
  total_output_tokens: 0,
  kind: "project",
};

function makeSession(id: string): Session {
  return {
    ...session,
    id,
    title: id,
    cwd: `/project/${id}`,
  };
}

function message(
  id: string,
  role: Message["role"],
  createdAt: number,
  overrides: Partial<Message> = {},
): Message {
  return {
    id,
    session_id: session.id,
    role,
    content: id,
    created_at: createdAt,
    ...overrides,
  };
}

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  const promise = new Promise<T>((next) => {
    resolve = next;
  });
  return { promise, resolve };
}

beforeEach(() => {
  invokeMock.mockReset();
  useChatStore.setState({
    sessions: [session],
    quickSessions: [],
    activeSession: null,
    draftSession: null,
    runtime: {},
    _unlisten: {},
    _unlistenSessionUpdated: {},
    _streamingMsgId: {},
    _selectionRequestId: 0,
  });
});

describe("long-session history paging", () => {
  it("selects a session from the bounded latest page instead of all messages", async () => {
    const latest: MessagePage = {
      messages: [message("u9", "user", 9), message("a9", "assistant", 10)],
      has_more: true,
      next_before_rowid: 90,
    };
    invokeMock.mockImplementation(async (command: string) => {
      if (command === "get_session") return session;
      if (command === "get_message_page") return latest;
      throw new Error(`unexpected command ${command}`);
    });

    await useChatStore.getState().selectSession(session.id);

    expect(invokeMock).not.toHaveBeenCalledWith("get_messages", expect.anything());
    expect(invokeMock).toHaveBeenCalledWith("get_message_page", {
      sessionId: session.id,
      beforeRowid: null,
      userTurnLimit: 8,
    });
    const runtime = useChatStore.getState().runtime[session.id]!;
    expect(runtime.messages.map((item) => item.id)).toEqual(["u9", "a9"]);
    expect(runtime.persistedMessages).toEqual(latest.messages);
    expect(runtime.historyBeforeRowid).toBe(90);
    expect(runtime.hasOlderHistory).toBe(true);
  });

  it("prepends an older page, refreshes the persisted tail, and ignores overlap", async () => {
    const latestRows = [message("u9", "user", 9), message("a9", "assistant", 10)];
    useChatStore.setState({
      activeSession: session,
      runtime: {
        [session.id]: {
          ...freshRuntime(
            latestRows.map((item) => ({
              id: item.id,
              role: item.role,
              content: item.content,
              createdAt: item.created_at,
            })),
            {
              persistedMessages: latestRows,
              historyBeforeRowid: 90,
              hasOlderHistory: true,
            },
          ),
        },
      },
    });
    const older: MessagePage = {
      messages: [message("u8", "user", 7), message("a8", "assistant", 8)],
      has_more: true,
      next_before_rowid: 70,
    };
    const refreshedLatest: MessagePage = {
      messages: [...latestRows, message("u10", "user", 11), message("a10", "assistant", 12)],
      has_more: true,
      next_before_rowid: 90,
    };
    invokeMock
      .mockResolvedValueOnce(older)
      .mockResolvedValueOnce(refreshedLatest);

    await useChatStore.getState().loadOlderMessages();

    const runtime = useChatStore.getState().runtime[session.id]!;
    expect(runtime.messages.map((item) => item.id)).toEqual([
      "u8",
      "a8",
      "u9",
      "a9",
      "u10",
      "a10",
    ]);
    expect(runtime.persistedMessages).toHaveLength(6);
    expect(runtime.historyBeforeRowid).toBe(70);
    expect(runtime.loadingOlderHistory).toBe(false);
  });

  it("keeps the faster B selection active when the earlier A page resolves late", async () => {
    const sessionA = makeSession("A");
    const sessionB = makeSession("B");
    const slowA = deferred<MessagePage>();
    const pageB: MessagePage = {
      messages: [
        message("u-b", "user", 1, { session_id: sessionB.id }),
        message("a-b", "assistant", 2, { session_id: sessionB.id }),
      ],
      has_more: false,
      next_before_rowid: null,
    };
    invokeMock.mockImplementation(
      async (command: string, args?: { sessionId?: string }) => {
        if (command === "get_session") {
          return args?.sessionId === sessionA.id ? sessionA : sessionB;
        }
        if (command === "get_message_page") {
          return args?.sessionId === sessionA.id ? slowA.promise : pageB;
        }
        throw new Error(`unexpected command ${command}`);
      },
    );
    useChatStore.setState({ sessions: [sessionA, sessionB] });

    const selectingA = useChatStore.getState().selectSession(sessionA.id);
    await vi.waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "get_message_page",
        expect.objectContaining({ sessionId: sessionA.id }),
      );
    });
    await useChatStore.getState().selectSession(sessionB.id);

    slowA.resolve({
      messages: [
        message("u-a", "user", 1, { session_id: sessionA.id }),
        message("a-a", "assistant", 2, { session_id: sessionA.id }),
      ],
      has_more: false,
      next_before_rowid: null,
    });
    await selectingA;

    const state = useChatStore.getState();
    expect(state.activeSession?.id).toBe(sessionB.id);
    expect(state.runtime[sessionB.id]?.messages.map((item) => item.id)).toEqual([
      "u-b",
      "a-b",
    ]);
    expect(state.runtime[sessionA.id]).toBeUndefined();
  });

  it("recovers a stale streaming bucket from the persisted final on re-selection", async () => {
    const stale = freshRuntime([
      {
        id: "live-user",
        role: "user",
        content: "request",
        createdAt: 1,
      },
      {
        id: "live-assistant",
        role: "assistant",
        content: "partial",
        createdAt: 2,
      },
    ]);
    stale.streaming = true;
    stale.revision = 7;
    useChatStore.setState({
      activeSession: session,
      runtime: { [session.id]: stale },
      _streamingMsgId: { [session.id]: "live-assistant" },
    });
    const persistedFinal: MessagePage = {
      messages: [
        message("db-user", "user", 1, { content: "request" }),
        message("db-final", "assistant", 3, { content: "persisted final" }),
      ],
      has_more: false,
      next_before_rowid: null,
    };
    invokeMock.mockImplementation(async (command: string) => {
      if (command === "get_session") return session;
      if (command === "is_chat_running") return false;
      if (command === "get_message_page") return persistedFinal;
      throw new Error(`unexpected command ${command}`);
    });

    await useChatStore.getState().selectSession(session.id);

    const state = useChatStore.getState();
    expect(invokeMock).toHaveBeenCalledWith("is_chat_running", {
      sessionId: session.id,
    });
    expect(state.runtime[session.id]?.streaming).toBe(false);
    expect(state.runtime[session.id]?.messages.map((item) => item.id)).toEqual([
      "db-user",
      "db-final",
    ]);
    expect(state.runtime[session.id]?.messages[1]?.content).toBe("persisted final");
    expect(state._streamingMsgId[session.id]).toBeUndefined();
  });

  it("does not hydrate over a streaming bucket that the backend reports active", async () => {
    const live = freshRuntime([
      {
        id: "live-assistant",
        role: "assistant",
        content: "still running",
        createdAt: 2,
      },
    ]);
    live.streaming = true;
    useChatStore.setState({
      activeSession: session,
      runtime: { [session.id]: live },
    });
    invokeMock.mockImplementation(async (command: string) => {
      if (command === "get_session") return session;
      if (command === "is_chat_running") return true;
      throw new Error(`unexpected command ${command}`);
    });

    await useChatStore.getState().selectSession(session.id);

    expect(useChatStore.getState().runtime[session.id]).toBe(live);
    expect(invokeMock).not.toHaveBeenCalledWith(
      "get_message_page",
      expect.anything(),
    );
  });

  it("drops a stale older-page result without overwriting a new live stream and releases loading", async () => {
    const latestRows = [message("u9", "user", 9), message("a9", "assistant", 10)];
    useChatStore.setState({
      activeSession: session,
      runtime: {
        [session.id]: freshRuntime(dbMessagesToUI(latestRows), {
          persistedMessages: latestRows,
          historyBeforeRowid: 90,
          hasOlderHistory: true,
        }),
      },
    });
    const slowOlder = deferred<MessagePage>();
    const slowLatest = deferred<MessagePage>();
    invokeMock
      .mockReturnValueOnce(slowOlder.promise)
      .mockReturnValueOnce(slowLatest.promise);

    const loading = useChatStore.getState().loadOlderMessages();
    expect(useChatStore.getState().runtime[session.id]?.loadingOlderHistory).toBe(true);

    const liveUser = {
      id: "live-user",
      role: "user" as const,
      content: "live request",
      createdAt: 11,
    };
    const liveAssistant = {
      id: "live-assistant",
      role: "assistant" as const,
      content: "live partial",
      createdAt: 12,
    };
    useChatStore.setState((state) => {
      const runtime = state.runtime[session.id]!;
      return {
        runtime: {
          ...state.runtime,
          [session.id]: {
            ...runtime,
            messages: [...runtime.messages, liveUser, liveAssistant],
            streaming: true,
            revision: runtime.revision + 1,
          },
        },
      };
    });

    slowOlder.resolve({
      messages: [message("u8", "user", 7), message("a8", "assistant", 8)],
      has_more: true,
      next_before_rowid: 70,
    });
    slowLatest.resolve({
      messages: latestRows,
      has_more: true,
      next_before_rowid: 90,
    });
    await loading;

    let runtime = useChatStore.getState().runtime[session.id]!;
    expect(runtime.messages.slice(-2)).toEqual([liveUser, liveAssistant]);
    expect(runtime.streaming).toBe(true);
    expect(runtime.loadingOlderHistory).toBe(false);
    expect(runtime.historyBeforeRowid).toBe(90);

    useChatStore.setState((state) => ({
      runtime: {
        ...state.runtime,
        [session.id]: {
          ...state.runtime[session.id]!,
          streaming: false,
          revision: state.runtime[session.id]!.revision + 1,
        },
      },
    }));
    invokeMock.mockReset();
    invokeMock
      .mockResolvedValueOnce({
        messages: [message("u8", "user", 7), message("a8", "assistant", 8)],
        has_more: false,
        next_before_rowid: null,
      } satisfies MessagePage)
      .mockResolvedValueOnce({
        messages: [
          ...latestRows,
          message("live-user-db", "user", 11),
          message("live-assistant-db", "assistant", 12),
        ],
        has_more: true,
        next_before_rowid: 90,
      } satisfies MessagePage);

    await useChatStore.getState().loadOlderMessages();

    runtime = useChatStore.getState().runtime[session.id]!;
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(runtime.loadingOlderHistory).toBe(false);
    expect(runtime.historyBeforeRowid).toBeNull();
  });

  it("preserves frontend-only local messages when an older persisted page rehydrates", async () => {
    const latestRows = [message("u9", "user", 9), message("a9", "assistant", 10)];
    useChatStore.setState({
      activeSession: session,
      runtime: {
        [session.id]: freshRuntime(dbMessagesToUI(latestRows), {
          persistedMessages: latestRows,
          historyBeforeRowid: 90,
          hasOlderHistory: true,
        }),
      },
    });
    useChatStore.getState().addLocalAssistantMessage("frontend-only notice");
    const local = useChatStore.getState().runtime[session.id]!.localMessages[0]!;
    invokeMock
      .mockResolvedValueOnce({
        messages: [message("u8", "user", 7), message("a8", "assistant", 8)],
        has_more: false,
        next_before_rowid: null,
      } satisfies MessagePage)
      .mockResolvedValueOnce({
        messages: latestRows,
        has_more: true,
        next_before_rowid: 90,
      } satisfies MessagePage);

    await useChatStore.getState().loadOlderMessages();

    const runtime = useChatStore.getState().runtime[session.id]!;
    expect(runtime.messages.map((item) => item.id)).toEqual([
      "u8",
      "a8",
      "u9",
      "a9",
      local.id,
    ]);
    expect(runtime.messages[runtime.messages.length - 1]).toBe(local);
    expect(runtime.localMessages).toEqual([local]);
    expect(runtime.persistedMessages.some((item) => item.id === local.id)).toBe(false);
  });

  it("rehydrates tool replay ownership and hides completion-gate internals across pages", async () => {
    const olderRows: Message[] = [
      message("u-old", "user", 1),
      message("decl-old", "assistant", 2, {
        content: "",
        tool_calls: JSON.stringify([
          {
            id: "call-old",
            type: "function",
            function: {
              name: "bash",
              arguments: JSON.stringify({ command: "printf old" }),
            },
          },
        ]),
      }),
      message("replay-old", "tool", 3, {
        content: JSON.stringify({
          tool_call_id: "call-old",
          content: "old result",
          status: "done",
        }),
      }),
      message("final-old", "assistant", 4, { content: "old final" }),
    ];
    const latestRows: Message[] = [
      message("u-new", "user", 5, { content: "new request" }),
      message("candidate-new", "assistant", 6, {
        content: "rejected candidate",
        completion_state: "rejected_candidate",
      }),
      message("recovery-new", "user", 7, {
        content: "internal recovery prompt",
        completion_state: "gate_recovery",
      }),
      message("decl-internal", "assistant", 8, {
        content: "internal validation",
        tool_calls: JSON.stringify([
          {
            id: "call-internal",
            type: "function",
            function: { name: "bash", arguments: "{}" },
          },
        ]),
      }),
      message("replay-internal", "tool", 9, {
        content: JSON.stringify({
          tool_call_id: "call-internal",
          content: "internal result",
          status: "done",
        }),
      }),
      message("ready-new", "user", 10, {
        content: "internal ready prompt",
        completion_state: "gate_ready",
      }),
      message("final-new", "assistant", 11, { content: "accepted final" }),
      message("notice-new", "system", 12, {
        content: "runtime notice",
        completion_state: "turn_notice",
      }),
    ];
    useChatStore.setState({
      activeSession: session,
      runtime: {
        [session.id]: freshRuntime(dbMessagesToUI(latestRows), {
          persistedMessages: latestRows,
          historyBeforeRowid: 50,
          hasOlderHistory: true,
        }),
      },
    });
    invokeMock
      .mockResolvedValueOnce({
        messages: olderRows,
        has_more: false,
        next_before_rowid: null,
      } satisfies MessagePage)
      .mockResolvedValueOnce({
        messages: latestRows,
        has_more: true,
        next_before_rowid: 50,
      } satisfies MessagePage);

    await useChatStore.getState().loadOlderMessages();

    const runtime = useChatStore.getState().runtime[session.id]!;
    expect(runtime.messages.map((item) => item.id)).toEqual([
      "u-old",
      "decl-old",
      "final-old",
      "u-new",
      "final-new",
      "notice-new",
    ]);
    expect(runtime.messages[1]?.toolCalls).toEqual([
      expect.objectContaining({
        id: "call-old",
        name: "bash",
        result: "old result",
        status: "done",
        isError: false,
      }),
    ]);
    expect(runtime.messages.map((item) => item.content)).not.toEqual(
      expect.arrayContaining([
        "rejected candidate",
        "internal recovery prompt",
        "internal validation",
        "internal result",
        "internal ready prompt",
      ]),
    );
    expect(runtime.messages[runtime.messages.length - 1]).toEqual(
      expect.objectContaining({
        role: "system",
        content: "runtime notice",
        completionState: "turn_notice",
      }),
    );
  });
});
