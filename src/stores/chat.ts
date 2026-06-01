// SPDX-License-Identifier: Apache-2.0
import { create } from "zustand";
import { invoke, onStream, onSessionUpdated, sendMessageAnonymous } from "../lib/tauri";
import type { Message, Session, StreamEvent, ModelInfo, ReasoningEffort, AnonTurn } from "../lib/tauri";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  markPermissionResponse,
  reduceChatStreamEvent,
  type PendingPermission,
  type ToolCallState,
  type UIMessage,
} from "./chatEvents";

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

interface ChatStore {
  sessions: Session[];
  /** Quick-task sessions (kind='quick'), kept separate from `sessions` so
   *  Home's "最近项目" stays project-only. The Workspace session sidebar
   *  merges both for its unified list. */
  quickSessions: Session[];
  activeSession: Session | null;
  messages: UIMessage[];
  streaming: boolean;
  models: ModelInfo[];
  activeModel: string;
  inputTokenTotal: number;
  outputTokenTotal: number;
  pendingPermission: PendingPermission | null;
  contextUsage: { used: number; limit: number } | null;
  compressionToast: { elidedCount: number; tokensFreed: number; id: number } | null;
  /** Messages waiting to fire after the current stream finishes. */
  queue: QueuedMessage[];

  loadSessions: () => Promise<void>;
  loadQuickSessions: () => Promise<void>;
  createSession: (cwd: string, model: string) => Promise<Session>;
  selectSession: (id: string) => Promise<void>;
  deleteSession: (id: string) => Promise<void>;
  sendMessage: (content: string) => Promise<void>;
  /** Send right now if idle, otherwise enqueue. Returns "sent" or "queued". */
  sendOrQueue: (content: string) => Promise<"sent" | "queued" | "full">;
  /** Remove a queued message before it fires. */
  removeFromQueue: (id: string) => void;
  /** Empty the queue without sending. */
  clearQueue: () => void;
  loadModels: (endpoint: string) => Promise<void>;
  setModel: (modelId: string) => void;
  cancelStream: () => void;
  respondPermission: (allow: boolean) => Promise<void>;
  addLocalAssistantMessage: (content: string) => void;
  clearVisibleConversation: () => void;
  updateActiveSessionModel: (modelId: string) => Promise<void>;
  updateActiveSessionReasoningEffort: (effort: ReasoningEffort | null) => Promise<void>;
  /** Begin an in-memory ANONYMOUS chat — never persisted, not learned from,
   *  API cost not counted. Returns the synthetic session so the caller can
   *  navigate to it (it lives only in this store, never in the DB). */
  startAnonymousSession: () => Session;
  /** Tear down the active anonymous chat, discarding its in-memory history. */
  exitAnonymous: () => void;

  _unlisten?: UnlistenFn;
  _unlistenSessionUpdated?: UnlistenFn;
  _streamingMsgId?: string;
}

export const useChatStore = create<ChatStore>((set, get) => ({
  sessions: [],
  quickSessions: [],
  activeSession: null,
  messages: [],
  streaming: false,
  models: [],
  activeModel: "anthropic/claude-opus-4-7",
  inputTokenTotal: 0,
  outputTokenTotal: 0,
  pendingPermission: null,
  contextUsage: null,
  compressionToast: null,
  queue: [],

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
    set((s) => ({ sessions: [session, ...s.sessions], activeSession: session, messages: [] }));
    return session;
  },

  selectSession: async (id) => {
    // An active anonymous session lives only in memory (never in the DB), so
    // re-selecting it must NOT hit get_session/get_messages — that would error
    // (no row) and wipe its in-memory history. Keep it as-is.
    const cur = get().activeSession;
    if (cur?.id === id && cur.kind === "anonymous") return;
    // Tear down the previous session's stream listeners so its events can't
    // bleed into the newly-selected session, and clear the global `streaming`
    // flag + queue. `streaming` is a single global flag, so a session left
    // mid-stream — or one whose terminal event was missed — would otherwise
    // keep every other session showing "running" and block all sends.
    // Switching sessions is therefore also the manual recovery path.
    get()._unlisten?.();
    get()._unlistenSessionUpdated?.();
    const session = await invoke<Session>("get_session", { sessionId: id });
    const msgs = await invoke<Message[]>("get_messages", { sessionId: id });
    set({
      activeSession: session,
      messages: msgs.map(dbToUI),
      activeModel: session.model_id,
      streaming: false,
      inputTokenTotal: 0,
      outputTokenTotal: 0,
      pendingPermission: null,
      contextUsage: null,
      compressionToast: null,
      queue: [],
      _unlisten: undefined,
      _unlistenSessionUpdated: undefined,
      _streamingMsgId: undefined,
    });
  },

  deleteSession: async (id) => {
    await invoke("delete_session", { sessionId: id });
    set((s) => ({
      sessions: s.sessions.filter((x) => x.id !== id),
      ...(s.activeSession?.id === id ? { activeSession: null, messages: [] } : {}),
    }));
  },

  sendMessage: async (content) => {
    const { activeSession, _unlisten, _unlistenSessionUpdated } = get();
    if (!activeSession || get().streaming) return;

    const isAnon = activeSession.kind === "anonymous";

    // Cancel any previous listeners
    _unlisten?.();
    _unlistenSessionUpdated?.();

    // Anonymous sessions are never persisted, so there's no server-side title
    // to subscribe to. Only normal sessions get the title-update listener.
    if (!isAnon) {
      const unlistenSessionUpdated = await onSessionUpdated(activeSession.id, (session) => {
        set((s) => ({
          activeSession: s.activeSession?.id === session.id ? session : s.activeSession,
          sessions: s.sessions.map((existing) =>
            existing.id === session.id ? session : existing
          ),
          // Quick sessions live in a separate array; mirror title/updated_at
          // changes there too so the sidebar's unified list stays fresh.
          quickSessions: s.quickSessions.map((existing) =>
            existing.id === session.id ? session : existing
          ),
        }));
      });
      set({ _unlistenSessionUpdated: unlistenSessionUpdated });
    }

    // Anonymous: snapshot the prior conversation from memory BEFORE appending
    // the new turn — the backend keeps no history, so the frontend replays it.
    const anonHistory: AnonTurn[] = isAnon
      ? get()
          .messages.filter(
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

    set((s) => ({
      messages: [...s.messages, userMsg, assistantMsg],
      streaming: true,
      _streamingMsgId: assistantMsgId,
    }));

    const unlisten = await onStream(activeSession.id, (event: StreamEvent) => {
      handleStreamEvent(event, assistantMsgId, set, get);
    });
    set({ _unlisten: unlisten });

    try {
      if (isAnon) {
        await sendMessageAnonymous(
          activeSession.id,
          content,
          anonHistory,
          activeSession.cwd,
          get().activeModel,
        );
      } else {
        await invoke("send_message", {
          sessionId: activeSession.id,
          content,
        });
      }
    } catch (e) {
      set((s) => ({
        messages: s.messages.map((m) =>
          m.id === assistantMsgId
            ? { ...m, content: `Error: ${String(e)}` }
            : m
        ),
        streaming: false,
      }));
    }
  },

  sendOrQueue: async (content) => {
    const text = content.trim();
    if (!text) return "sent";
    const { streaming, queue, sendMessage } = get();
    if (!streaming) {
      await sendMessage(text);
      return "sent";
    }
    if (queue.length >= QUEUE_MAX) return "full";
    set((s) => ({
      queue: [
        ...s.queue,
        { id: crypto.randomUUID(), content: text, enqueuedAt: Date.now() },
      ],
    }));
    return "queued";
  },

  removeFromQueue: (id) => {
    set((s) => ({ queue: s.queue.filter((q) => q.id !== id) }));
  },

  clearQueue: () => set({ queue: [] }),

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
      sessions: s.sessions.map((existing) =>
        existing.id === session.id ? session : existing,
      ),
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
      sessions: s.sessions.map((existing) =>
        existing.id === session.id ? session : existing,
      ),
    }));
  },

  startAnonymousSession: () => {
    // A purely in-memory session: a client-generated id, kind "anonymous", and
    // an empty cwd (the backend resolves a scratch dir). Never written to the
    // DB. Replaces the visible conversation with a fresh, blank one.
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
    get()._unlisten?.();
    get()._unlistenSessionUpdated?.();
    set({
      activeSession: anon,
      messages: [],
      streaming: false,
      queue: [],
      inputTokenTotal: 0,
      outputTokenTotal: 0,
      pendingPermission: null,
      contextUsage: null,
      compressionToast: null,
      _unlisten: undefined,
      _unlistenSessionUpdated: undefined,
    });
    return anon;
  },

  exitAnonymous: () => {
    if (get().activeSession?.kind !== "anonymous") return;
    get()._unlisten?.();
    get()._unlistenSessionUpdated?.();
    // Drop the in-memory session + its history entirely (no trace kept).
    set({
      activeSession: null,
      messages: [],
      streaming: false,
      queue: [],
      inputTokenTotal: 0,
      outputTokenTotal: 0,
      pendingPermission: null,
      contextUsage: null,
      compressionToast: null,
      _unlisten: undefined,
      _unlistenSessionUpdated: undefined,
    });
  },

  cancelStream: () => {
    const id = get().activeSession?.id;
    get()._unlisten?.();
    set({ streaming: false, _unlisten: undefined, pendingPermission: null });
    // Also tell the backend to stop the in-flight turn — otherwise the agent
    // keeps looping (burning tokens) after the UI already says "stopped".
    // Cooperative: it stops between rounds, never mid tool-call. Scoped to THIS
    // chat session only; it never affects the task scheduler / long task runs.
    if (id) void invoke("cancel_chat", { sessionId: id });
  },

  respondPermission: async (allow) => {
    const pending = get().pendingPermission;
    if (!pending) return;
    await invoke("respond_to_permission", {
      toolCallId: pending.toolCallId,
      allow,
    });
    set((s) => markPermissionResponse(s, pending.toolCallId, allow));
  },

  addLocalAssistantMessage: (content) => {
    const msg: UIMessage = {
      id: crypto.randomUUID(),
      role: "assistant",
      content,
      createdAt: Date.now(),
    };
    set((s) => ({ messages: [...s.messages, msg] }));
  },

  clearVisibleConversation: () => {
    set({
      messages: [],
      inputTokenTotal: 0,
      outputTokenTotal: 0,
      pendingPermission: null,
      contextUsage: null,
      compressionToast: null,
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

function handleStreamEvent(
  event: StreamEvent,
  msgId: string,
  set: (fn: (s: ChatStore) => Partial<ChatStore>) => void,
  get: () => ChatStore
) {
  const wasStreaming = get().streaming;
  set((s) => reduceChatStreamEvent(s, event, msgId));
  // Queue drain — fire the next queued message as soon as the previous
  // stream lands in a terminal state (done OR error). We delay one tick
  // so the just-completed sendMessage's React state has a chance to
  // settle and we don't re-enter mid-reducer-update.
  if (wasStreaming && !get().streaming) {
    const next = get().queue[0];
    if (next) {
      get().removeFromQueue(next.id);
      setTimeout(() => {
        void get().sendMessage(next.content);
      }, 0);
      return; // more conversation coming — defer post-mortem
    }

    // Chat-end post-mortem trigger. The Workspace already fires this
    // after task trees settle (see stores/tasks.ts); here we cover
    // free-form chat sessions too, since those produce just as many
    // signals worth learning from. Guards:
    //   - throttled per session to avoid burning tokens on rapid replies
    //   - skip too-short conversations (< 3 messages = noise)
    //   - skip 'error' terminations (the task path skips failures too)
    //   - NEVER for anonymous chats — they must not feed the learning/profile
    //     pipeline (no-trace = no learning).
    const session = get().activeSession;
    if (
      session &&
      session.kind !== "anonymous" &&
      event.type === "done" &&
      get().messages.length >= POSTMORTEM_MIN_MESSAGES
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
