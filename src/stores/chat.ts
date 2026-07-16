// SPDX-License-Identifier: Apache-2.0
import { create } from "zustand";
import { invoke, onStream, onSessionUpdated, sendMessageAnonymous } from "../lib/tauri";
import type { Message, Session, StreamEvent, ModelInfo, ReasoningEffort, AnonTurn } from "../lib/tauri";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  markPermissionResponse,
  reduceChatStreamEvent,
  formatToolArgs,
  type ChatEventState,
  type PendingPermission,
  type ToolCallState,
  type UIMessage,
} from "./chatEvents";
import { useSettingsStore } from "./settings";

export type { PendingPermission, ToolCallState, UIMessage };

/** A message queued while the assistant was streaming. Drains FIFO when
 *  streaming flips back to false. Capped client-side at QUEUE_MAX to
 *  prevent runaway accidental enqueue. */
export interface QueuedMessage {
  id: string;
  content: string;
  /** Future: attachments. Kept optional so existing call sites need no change. */
  enqueuedAt: number;
}

export const QUEUE_MAX = 5;

/** All ephemeral chat state for ONE session. Buckets are keyed by session id in
 *  the store, so multiple sessions can stream concurrently without clobbering
 *  each other — each has its own messages, streaming flag, queue, stats, and
 *  (privately) its own stream listener. */
export interface SessionRuntime extends ChatEventState {
  /** Messages waiting to fire after THIS session's current stream finishes. */
  queue: QueuedMessage[];
}

/** Shared read-only default returned by the selector when a session has no
 *  bucket yet (e.g. no active session). Never mutated. */
const EMPTY_RUNTIME: SessionRuntime = {
  messages: [],
  streaming: false,
  inputTokenTotal: 0,
  outputTokenTotal: 0,
  pendingPermission: null,
  contextUsage: null,
  compressionToast: null,
  queue: [],
};

/** A fresh, independent runtime bucket (its own arrays). */
export function freshRuntime(messages: UIMessage[] = []): SessionRuntime {
  return {
    messages,
    streaming: false,
    inputTokenTotal: 0,
    outputTokenTotal: 0,
    pendingPermission: null,
    contextUsage: null,
    compressionToast: null,
    queue: [],
  };
}

interface ChatStore {
  sessions: Session[];
  quickSessions: Session[];
  activeSession: Session | null;
  /** Per-session ephemeral chat state, keyed by session id. Source of truth for
   *  messages / streaming / queue / token + context stats. Multiple sessions
   *  can be present and streaming at the same time. */
  runtime: Record<string, SessionRuntime>;
  models: ModelInfo[];
  activeModel: string;

  loadSessions: () => Promise<void>;
  loadQuickSessions: () => Promise<void>;
  createSession: (cwd: string, model: string) => Promise<Session>;
  selectSession: (id: string) => Promise<void>;
  deleteSession: (id: string) => Promise<void>;
  renameSession: (id: string, title: string) => Promise<void>;
  /** Send to `sessionId` (default: the active session). Targeting lets the
   *  queue-drain fire the next message into a background session too. */
  sendMessage: (content: string, sessionId?: string) => Promise<void>;
  /** Send right now if the active session is idle, otherwise enqueue. */
  sendOrQueue: (content: string) => Promise<"sent" | "queued" | "full">;
  /** Remove a queued message (active session) before it fires. */
  removeFromQueue: (id: string) => void;
  /** Empty the active session's queue without sending. */
  clearQueue: () => void;
  loadModels: (endpoint: string) => Promise<void>;
  setModel: (modelId: string) => void;
  /** Stop the in-flight turn for `sessionId` (default: the active session). */
  cancelStream: (sessionId?: string) => void;
  respondPermission: (allow: boolean) => Promise<void>;
  addLocalAssistantMessage: (content: string) => void;
  clearVisibleConversation: () => void;
  updateActiveSessionModel: (modelId: string) => Promise<void>;
  updateActiveSessionReasoningEffort: (effort: ReasoningEffort | null) => Promise<void>;
  startAnonymousSession: () => Session;
  exitAnonymous: () => void;

  /** Per-session stream listeners — kept alive across session switches so a
   *  background session keeps streaming into its own runtime bucket. */
  _unlisten: Record<string, UnlistenFn | undefined>;
  _unlistenSessionUpdated: Record<string, UnlistenFn | undefined>;
  _streamingMsgId: Record<string, string | undefined>;
}

/** Selector: the active session's runtime slice (or the empty default). Use as
 *  `useChatStore(activeRuntime)`. */
export function activeRuntime(s: ChatStore): SessionRuntime {
  const id = s.activeSession?.id;
  return (id ? s.runtime[id] : undefined) ?? EMPTY_RUNTIME;
}

/** Resolve a session object by id across the active / project / quick lists. */
function findSession(s: ChatStore, id: string): Session | undefined {
  if (s.activeSession?.id === id) return s.activeSession;
  return s.sessions.find((x) => x.id === id) ?? s.quickSessions.find((x) => x.id === id);
}

export const useChatStore = create<ChatStore>((set, get) => ({
  sessions: [],
  quickSessions: [],
  activeSession: null,
  runtime: {},
  models: [],
  activeModel: "anthropic/claude-opus-4-7",
  _unlisten: {},
  _unlistenSessionUpdated: {},
  _streamingMsgId: {},

  loadSessions: async () => {
    const sessions = await invoke<Session[]>("list_sessions");
    set({ sessions });
  },

  loadQuickSessions: async () => {
    const quickSessions = await invoke<Session[]>("list_quick_sessions");
    set({ quickSessions });
  },

  createSession: async (cwd, model) => {
    const title = cwd.split(/[/\\]/).pop() ?? "New Session";
    const session = await invoke<Session>("create_session", { title, cwd, modelId: model });
    set((s) => ({
      sessions: [session, ...s.sessions],
      activeSession: session,
      activeModel: session.model_id,
      runtime: { ...s.runtime, [session.id]: freshRuntime() },
    }));
    return session;
  },

  selectSession: async (id) => {
    // Re-selecting the active anonymous session: it lives only in memory, so
    // never hit get_session/get_messages (that would error and wipe history).
    const cur = get().activeSession;
    if (cur?.id === id && cur.kind === "anonymous") return;

    const session = await invoke<Session>("get_session", { sessionId: id });

    // If this session is mid-stream it already owns a live bucket + listener —
    // foreground it WITHOUT reloading (a reload would clobber the in-flight
    // buffer and drop the live tail).
    if (get().runtime[id]?.streaming) {
      set({ activeSession: session, activeModel: session.model_id });
      return;
    }

    // Otherwise load a fresh snapshot and (re)seed this session's bucket.
    const msgs = await invoke<Message[]>("get_messages", { sessionId: id });
    set((s) => ({
      activeSession: session,
      activeModel: session.model_id,
      runtime: { ...s.runtime, [id]: freshRuntime(dbMessagesToUI(msgs)) },
    }));
  },

  deleteSession: async (id) => {
    await invoke("delete_session", { sessionId: id });
    get()._unlisten[id]?.();
    get()._unlistenSessionUpdated[id]?.();
    set((s) => {
      const runtime = { ...s.runtime };
      delete runtime[id];
      const _unlisten = { ...s._unlisten };
      delete _unlisten[id];
      const _unlistenSessionUpdated = { ...s._unlistenSessionUpdated };
      delete _unlistenSessionUpdated[id];
      const _streamingMsgId = { ...s._streamingMsgId };
      delete _streamingMsgId[id];
      return {
        sessions: s.sessions.filter((x) => x.id !== id),
        quickSessions: s.quickSessions.filter((x) => x.id !== id),
        runtime,
        _unlisten,
        _unlistenSessionUpdated,
        _streamingMsgId,
        ...(s.activeSession?.id === id ? { activeSession: null } : {}),
      };
    });
  },

  renameSession: async (id, title) => {
    await invoke("update_session_title", { sessionId: id, title });
    set((s) => ({
      sessions: s.sessions.map((x) => (x.id === id ? { ...x, title } : x)),
      quickSessions: s.quickSessions.map((x) => (x.id === id ? { ...x, title } : x)),
      ...(s.activeSession?.id === id
        ? { activeSession: { ...s.activeSession, title } }
        : {}),
    }));
  },

  sendMessage: async (content, sessionId) => {
    const target = sessionId ? findSession(get(), sessionId) : get().activeSession;
    if (!target) return;
    const id = target.id;
    if (get().runtime[id]?.streaming) return;

    const isAnon = target.kind === "anonymous";

    // Re-subscribe THIS session's listeners (tear down any stale ones first).
    get()._unlisten[id]?.();
    get()._unlistenSessionUpdated[id]?.();

    // Anonymous sessions are never persisted → no server-side title to track.
    if (!isAnon) {
      const unlistenSessionUpdated = await onSessionUpdated(id, (session) => {
        set((s) => ({
          activeSession: s.activeSession?.id === session.id ? session : s.activeSession,
          activeModel: s.activeSession?.id === session.id ? session.model_id : s.activeModel,
          sessions: s.sessions.map((existing) => (existing.id === session.id ? session : existing)),
          quickSessions: s.quickSessions.map((existing) =>
            existing.id === session.id ? session : existing,
          ),
        }));
      });
      set((s) => ({
        _unlistenSessionUpdated: { ...s._unlistenSessionUpdated, [id]: unlistenSessionUpdated },
      }));
    }

    // Anonymous: replay this session's in-memory history (backend keeps none).
    const anonHistory: AnonTurn[] = isAnon
      ? (get().runtime[id]?.messages ?? [])
          .filter(
            (m) => (m.role === "user" || m.role === "assistant") && m.content.trim().length > 0,
          )
          .map((m) => ({ role: m.role as "user" | "assistant", content: m.content }))
      : [];

    const userMsg: UIMessage = {
      id: crypto.randomUUID(),
      role: "user",
      content,
      createdAt: Date.now(),
    };
    const assistantMsgId = crypto.randomUUID();
    const assistantMsg: UIMessage = {
      id: assistantMsgId,
      role: "assistant",
      content: "",
      toolCalls: [],
      createdAt: Date.now(),
    };

    set((s) => {
      const prev = s.runtime[id] ?? freshRuntime();
      return {
        runtime: {
          ...s.runtime,
          [id]: { ...prev, messages: [...prev.messages, userMsg, assistantMsg], streaming: true },
        },
        _streamingMsgId: { ...s._streamingMsgId, [id]: assistantMsgId },
      };
    });

    const unlisten = await onStream(id, (event: StreamEvent) => {
      handleStreamEvent(event, id, assistantMsgId, set, get);
    });
    set((s) => ({ _unlisten: { ...s._unlisten, [id]: unlisten } }));

    try {
      if (isAnon) {
        await sendMessageAnonymous(id, content, anonHistory, target.cwd, target.model_id);
      } else {
        await invoke("send_message", { sessionId: id, content });
      }
    } catch (e) {
      set((s) => {
        const prev = s.runtime[id];
        if (!prev) return {};
        return {
          runtime: {
            ...s.runtime,
            [id]: {
              ...prev,
              messages: prev.messages.map((m) =>
                m.id === assistantMsgId ? { ...m, content: `Error: ${String(e)}` } : m,
              ),
              streaming: false,
            },
          },
        };
      });
    }
  },

  sendOrQueue: async (content) => {
    const text = content.trim();
    if (!text) return "sent";
    const active = get().activeSession;
    if (!active) return "sent";
    const id = active.id;
    const rt = get().runtime[id];
    if (!rt || !rt.streaming) {
      await get().sendMessage(text);
      return "sent";
    }
    if (rt.queue.length >= QUEUE_MAX) return "full";
    set((s) => {
      const prev = s.runtime[id] ?? freshRuntime();
      return {
        runtime: {
          ...s.runtime,
          [id]: {
            ...prev,
            queue: [
              ...prev.queue,
              { id: crypto.randomUUID(), content: text, enqueuedAt: Date.now() },
            ],
          },
        },
      };
    });
    return "queued";
  },

  removeFromQueue: (qid) => {
    const id = get().activeSession?.id;
    if (!id) return;
    set((s) => {
      const prev = s.runtime[id];
      if (!prev) return {};
      return {
        runtime: { ...s.runtime, [id]: { ...prev, queue: prev.queue.filter((q) => q.id !== qid) } },
      };
    });
  },

  clearQueue: () => {
    const id = get().activeSession?.id;
    if (!id) return;
    set((s) => {
      const prev = s.runtime[id];
      if (!prev) return {};
      return { runtime: { ...s.runtime, [id]: { ...prev, queue: [] } } };
    });
  },

  loadModels: async (endpoint) => {
    try {
      const models = await invoke<ModelInfo[]>("list_models", { endpointName: endpoint });
      set({ models });
    } catch {
      // silently ignore — user hasn't set key yet
    }
  },

  setModel: (modelId) => set({ activeModel: modelId }),

  updateActiveSessionModel: async (modelId) => {
    const activeSession = get().activeSession;
    set({ activeModel: modelId });
    if (!activeSession) return;
    if (activeSession.kind === "anonymous") {
      // No DB row to persist to — reflect the choice on the in-memory session.
      set({ activeSession: { ...activeSession, model_id: modelId } });
      return;
    }

    const session = await invoke<Session>("update_session_model", {
      sessionId: activeSession.id,
      modelId,
    });
    set((s) => ({
      activeSession: session,
      sessions: s.sessions.map((existing) => (existing.id === session.id ? session : existing)),
    }));
  },

  updateActiveSessionReasoningEffort: async (effort) => {
    const activeSession = get().activeSession;
    if (!activeSession) return;
    if (activeSession.kind === "anonymous") {
      set({ activeSession: { ...activeSession, reasoning_effort: effort } });
      return;
    }
    const session = await invoke<Session>("update_session_reasoning_effort", {
      sessionId: activeSession.id,
      effort,
    });
    set((s) => ({
      activeSession: session,
      sessions: s.sessions.map((existing) => (existing.id === session.id ? session : existing)),
    }));
  },

  startAnonymousSession: () => {
    // A purely in-memory session: a client-generated id, kind "anonymous", and
    // an empty cwd (the backend resolves a scratch dir). Never written to the
    // DB. Gets its own fresh runtime bucket like any other session.
    const anon: Session = {
      id: crypto.randomUUID(),
      title: "匿名会话",
      cwd: "",
      model_id: get().activeModel,
      created_at: Date.now(),
      updated_at: Date.now(),
      total_input_tokens: 0,
      total_output_tokens: 0,
      kind: "anonymous",
    };
    set((s) => ({
      activeSession: anon,
      runtime: { ...s.runtime, [anon.id]: freshRuntime() },
    }));
    return anon;
  },

  exitAnonymous: () => {
    const active = get().activeSession;
    if (active?.kind !== "anonymous") return;
    const id = active.id;
    // Drop the in-memory session, its history, and its listeners entirely.
    get()._unlisten[id]?.();
    get()._unlistenSessionUpdated[id]?.();
    set((s) => {
      const runtime = { ...s.runtime };
      delete runtime[id];
      const _unlisten = { ...s._unlisten };
      delete _unlisten[id];
      const _unlistenSessionUpdated = { ...s._unlistenSessionUpdated };
      delete _unlistenSessionUpdated[id];
      const _streamingMsgId = { ...s._streamingMsgId };
      delete _streamingMsgId[id];
      return { activeSession: null, runtime, _unlisten, _unlistenSessionUpdated, _streamingMsgId };
    });
  },

  cancelStream: (sessionId) => {
    const id = sessionId ?? get().activeSession?.id;
    if (!id) return;
    set((s) => {
      const prev = s.runtime[id];
      const runtime = prev
        ? { ...s.runtime, [id]: { ...prev, pendingPermission: null } }
        : s.runtime;
      return { runtime };
    });
    // Tell the backend to stop the in-flight turn — otherwise the agent keeps
    // looping (burning tokens) after the UI already says "stopped". Cooperative:
    // it stops between rounds, never mid tool-call. Scoped to THIS chat session
    // only; it never affects the task scheduler / long task runs.
    // Keep the listener and streaming state until the backend emits Done. That
    // terminal event is the only safe point to drain a queued message; sending
    // it immediately would race the still-running tool call in the old turn.
    void invoke("cancel_chat", { sessionId: id });
  },

  respondPermission: async (allow) => {
    const id = get().activeSession?.id;
    if (!id) return;
    const pending = get().runtime[id]?.pendingPermission;
    if (!pending) return;
    await invoke("respond_to_permission", { toolCallId: pending.toolCallId, allow });
    set((s) => {
      const prev = s.runtime[id];
      if (!prev) return {};
      return {
        runtime: {
          ...s.runtime,
          [id]: { ...prev, ...markPermissionResponse(prev, pending.toolCallId, allow) },
        },
      };
    });
  },

  addLocalAssistantMessage: (content) => {
    const id = get().activeSession?.id;
    if (!id) return;
    const msg: UIMessage = {
      id: crypto.randomUUID(),
      role: "assistant",
      content,
      createdAt: Date.now(),
    };
    set((s) => {
      const prev = s.runtime[id] ?? freshRuntime();
      return { runtime: { ...s.runtime, [id]: { ...prev, messages: [...prev.messages, msg] } } };
    });
  },

  clearVisibleConversation: () => {
    const id = get().activeSession?.id;
    if (!id) return;
    set((s) => {
      const prev = s.runtime[id];
      if (!prev) return {};
      return {
        runtime: {
          ...s.runtime,
          [id]: {
            ...prev,
            messages: [],
            inputTokenTotal: 0,
            outputTokenTotal: 0,
            pendingPermission: null,
            contextUsage: null,
            compressionToast: null,
          },
        },
      };
    });
  },
}));

/** Throttle: don't fire post-mortem more than once every 5 minutes per
 *  session. Saves tokens during back-and-forth chats. */
const POSTMORTEM_THROTTLE_MS = 5 * 60 * 1000;
/** Skip post-mortem unless the conversation has at least this many
 *  messages — anything shorter has nothing useful to learn from. */
const POSTMORTEM_MIN_MESSAGES = 3;
const _lastPostmortemAt: Record<string, number> = {};

function drainNextQueuedMessage(
  sessionId: string,
  set: (fn: (s: ChatStore) => Partial<ChatStore>) => void,
  get: () => ChatStore,
): boolean {
  const next = get().runtime[sessionId]?.queue[0];
  if (!next) return false;

  set((s) => {
    const prev = s.runtime[sessionId];
    if (!prev) return {};
    return {
      runtime: {
        ...s.runtime,
        [sessionId]: { ...prev, queue: prev.queue.filter((q) => q.id !== next.id) },
      },
    };
  });
  setTimeout(() => {
    void get().sendMessage(next.content, sessionId);
  }, 0);
  return true;
}

function handleStreamEvent(
  event: StreamEvent,
  sessionId: string,
  msgId: string,
  set: (fn: (s: ChatStore) => Partial<ChatStore>) => void,
  get: () => ChatStore,
) {
  const wasStreaming = get().runtime[sessionId]?.streaming ?? false;
  set((s) => {
    const prev = s.runtime[sessionId];
    if (!prev) return {};
    const reduced = reduceChatStreamEvent(prev, event, msgId);
    return { runtime: { ...s.runtime, [sessionId]: { ...prev, ...reduced } } };
  });

  // Queue drain — fire this session's next queued message as soon as its
  // stream lands in a terminal state (done OR error). We delay one tick so the
  // just-completed send's React state settles before we re-enter.
  const nowStreaming = get().runtime[sessionId]?.streaming ?? false;
  if (wasStreaming && !nowStreaming) {
    if (drainNextQueuedMessage(sessionId, set, get)) {
      return; // more conversation coming — defer post-mortem
    }

    // Chat-end post-mortem trigger. Mirrors the task path (stores/tasks.ts).
    // Guards: throttled per session, skip too-short conversations, skip
    // 'error' terminations, and NEVER for anonymous chats (no-trace = no
    // learning).
    const session = findSession(get(), sessionId);
    if (
      session &&
      session.kind !== "anonymous" &&
      event.type === "done" &&
      useSettingsStore.getState().settings?.remote_postmortem_enabled === true &&
      (get().runtime[sessionId]?.messages.length ?? 0) >= POSTMORTEM_MIN_MESSAGES
    ) {
      const last = _lastPostmortemAt[session.id] ?? 0;
      if (Date.now() - last >= POSTMORTEM_THROTTLE_MS) {
        _lastPostmortemAt[session.id] = Date.now();
        // Fire-and-forget; backend errors are logged but never block UX.
        invoke("run_postmortem", {
          sessionId: session.id,
          cwd: session.cwd,
        }).catch((e) => {
          // eslint-disable-next-line no-console
          console.warn("chat postmortem failed (non-fatal)", e);
        });
      }
    }
  }
}

interface PersistedToolCall {
  id?: unknown;
  function?: {
    name?: unknown;
    arguments?: unknown;
  };
}

interface PersistedToolReplay {
  tool_call_id?: unknown;
  content?: unknown;
  status?: unknown;
}

function parsePersistedToolCalls(raw: string | null | undefined): ToolCallState[] {
  if (!raw) return [];
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed.flatMap((value) => {
      const call = value as PersistedToolCall;
      if (typeof call.id !== "string" || typeof call.function?.name !== "string") return [];
      const rawArgs = call.function.arguments;
      let args: unknown = rawArgs ?? {};
      if (typeof rawArgs === "string") {
        try {
          args = JSON.parse(rawArgs);
        } catch {
          args = rawArgs;
        }
      }
      return [
        {
          id: call.id,
          name: call.function.name,
          args: formatToolArgs(args),
          status: "running" as const,
        },
      ];
    });
  } catch {
    return [];
  }
}

function parsePersistedToolReplay(raw: string): {
  toolCallId: string;
  content: string;
  status: "done" | "error" | "denied";
} | null {
  try {
    const replay = JSON.parse(raw) as PersistedToolReplay;
    if (typeof replay.tool_call_id !== "string" || typeof replay.content !== "string") {
      return null;
    }
    const status =
      replay.status === "error" || replay.status === "denied" || replay.status === "done"
        ? replay.status
        : "done";
    return { toolCallId: replay.tool_call_id, content: replay.content, status };
  } catch {
    return null;
  }
}

/** Rebuild persisted tool cards and fold role=tool replay rows into the
 * assistant declaration that owns them. Provider replay rows are transport
 * history, not standalone chat bubbles. */
export function dbMessagesToUI(messages: Message[]): UIMessage[] {
  const hydrated: UIMessage[] = [];
  const toolOwners = new Map<string, number>();

  for (const message of messages) {
    if (message.role === "tool") {
      const replay = parsePersistedToolReplay(message.content);
      const ownerIndex = replay ? toolOwners.get(replay.toolCallId) : undefined;
      if (replay && ownerIndex != null) {
        const owner = hydrated[ownerIndex];
        owner.toolCalls = owner.toolCalls?.map((call) =>
          call.id === replay.toolCallId
            ? {
                ...call,
                result: replay.content,
                status: replay.status,
                isError: replay.status !== "done",
              }
            : call,
        );
      }
      continue;
    }

    const uiMessage = dbToUI(message);
    if (message.role === "assistant") {
      const toolCalls = parsePersistedToolCalls(message.tool_calls);
      if (toolCalls.length > 0) {
        uiMessage.toolCalls = toolCalls;
        const ownerIndex = hydrated.length;
        for (const call of toolCalls) toolOwners.set(call.id, ownerIndex);
      }
    }
    hydrated.push(uiMessage);
  }

  return hydrated;
}

function dbToUI(m: Message): UIMessage {
  return {
    id: m.id,
    role: m.role,
    content: m.content,
    inputTokens: m.input_tokens,
    outputTokens: m.output_tokens,
    createdAt: m.created_at,
  };
}
