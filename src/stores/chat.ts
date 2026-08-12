// SPDX-License-Identifier: Apache-2.0
import { create } from "zustand";
import { invoke, onStream, onSessionUpdated, sendMessageAnonymous } from "../lib/tauri";
import type {
  Message,
  MessagePage,
  Session,
  StreamEvent,
  ModelInfo,
  ReasoningEffort,
  PermissionMode,
  TurnPlanSnapshot,
  TurnActivitySnapshot,
  AnonTurn,
} from "../lib/tauri";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { turnPlanFromEvent } from "../lib/chatPlan";
import {
  markPermissionResponse,
  reduceChatStreamEvent,
  formatToolArgs,
  presentChatInvocationError,
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
  /** Exact persisted root identity for a materialized draft's first message.
   * Preserve it if a busy backend makes the frontend restore the message. */
  rootTurnId?: string;
}

/** A conversation shell that exists only in frontend memory until the first
 * real message is submitted. It must never be passed to backend session APIs.
 *
 * A draft has exactly two switches, both changeable right up to the moment the
 * first message is sent:
 *   • `cwd`       — which project directory to work in (null = standalone task)
 *   • `anonymous` — leave no trace (never persisted, never listed)
 *
 * There is deliberately no "kind" here. Picking a project is choosing *where*
 * this conversation works, never *which* conversation to open — that
 * distinction is the whole point of the draft. */
export interface DraftSession {
  id: string;
  cwd: string | null;
  anonymous: boolean;
  modelId: string;
  permissionMode?: PermissionMode;
  text: string;
  /** Bound on the first send attempt and retained across uncertain retries. */
  firstMessageId?: string;
}

export const QUEUE_MAX = 5;

/** All ephemeral chat state for ONE session. Buckets are keyed by session id in
 *  the store, so multiple sessions can stream concurrently without clobbering
 *  each other — each has its own messages, streaming flag, queue, stats, and
 *  (privately) its own stream listener. */
export interface SessionRuntime extends ChatEventState {
  /** Messages waiting to fire after THIS session's current stream finishes. */
  queue: QueuedMessage[];
  /** Raw persisted rows already loaded for this bounded history window. */
  persistedMessages: Message[];
  /** Latest structured plan snapshot per loaded real user turn. */
  persistedPlans: TurnPlanSnapshot[];
  /** Stable SQLite rowid cursor for the next older real-user-turn page. */
  historyBeforeRowid: number | null;
  hasOlderHistory: boolean;
  loadingOlderHistory: boolean;
  /** At least one loaded page was split or previewed by a safety budget. */
  historyTruncated: boolean;
  /** Guards async history refreshes from overwriting a newer live turn. */
  revision: number;
  /** Unique owner of the current history request; avoids stale cleanup ABA. */
  historyRequestId: number;
  /** Frontend-only notices that must survive persisted history hydration. */
  localMessages: UIMessage[];
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
  transportDoneSucceeded: false,
  queue: [],
  persistedMessages: [],
  persistedPlans: [],
  historyBeforeRowid: null,
  hasOlderHistory: false,
  loadingOlderHistory: false,
  historyTruncated: false,
  revision: 0,
  historyRequestId: 0,
  localMessages: [],
};

/** A fresh, independent runtime bucket (its own arrays). */
export function freshRuntime(
  messages: UIMessage[] = [],
  history: Partial<
    Pick<
      SessionRuntime,
      "persistedMessages" | "persistedPlans" | "historyBeforeRowid" | "hasOlderHistory" | "historyTruncated"
    >
  > = {},
): SessionRuntime {
  return {
    messages,
    streaming: false,
    inputTokenTotal: 0,
    outputTokenTotal: 0,
    pendingPermission: null,
    contextUsage: null,
    compressionToast: null,
    transportDoneSucceeded: false,
    queue: [],
    persistedMessages: history.persistedMessages ?? [],
    persistedPlans: history.persistedPlans ?? [],
    historyBeforeRowid: history.historyBeforeRowid ?? null,
    hasOlderHistory: history.hasOlderHistory ?? false,
    loadingOlderHistory: false,
    historyTruncated: history.historyTruncated ?? false,
    revision: 0,
    historyRequestId: 0,
    localMessages: [],
  };
}

interface ChatStore {
  /** Every session the user owns, newest first — project and standalone alike.
   *  Grouping into projects is a view concern (see `lib/projects`). */
  sessions: Session[];
  activeSession: Session | null;
  draftSession: DraftSession | null;
  /** Open a brand-new empty conversation. Optionally scoped to a project
   *  directory and/or anonymous. Never touches the backend. */
  beginDraft: (opts?: { cwd?: string | null; anonymous?: boolean }) => DraftSession;
  /** Re-scope the current draft to a project (or back to standalone). This is
   *  the ONLY thing picking a project does — it never opens a session. */
  setDraftProject: (cwd: string | null) => void;
  setDraftAnonymous: (anonymous: boolean) => void;
  updateDraftText: (text: string) => void;
  discardDraft: () => void;
  /** Per-session ephemeral chat state, keyed by session id. Source of truth for
   *  messages / streaming / queue / token + context stats. Multiple sessions
   *  can be present and streaming at the same time. */
  runtime: Record<string, SessionRuntime>;
  models: ModelInfo[];
  activeModel: string;

  loadSessions: () => Promise<Session[]>;
  createSession: (cwd: string, model: string) => Promise<Session>;
  selectSession: (id: string) => Promise<void>;
  loadOlderMessages: () => Promise<void>;
  deleteSession: (id: string) => Promise<void>;
  renameSession: (id: string, title: string) => Promise<void>;
  /** Send to `sessionId` (default: the active session). Targeting lets the
   *  queue-drain fire the next message into a background session too. */
  sendMessage: (content: string, sessionId?: string, rootTurnId?: string) => Promise<void>;
  /** Send right now if idle, enqueue if streaming, or materialize an active draft. */
  sendOrQueue: (content: string) => Promise<"sent" | "queued" | "full" | "failed">;
  /** Remove a queued message (active session) before it fires. */
  removeFromQueue: (id: string) => void;
  /** Empty the active session's queue without sending. */
  clearQueue: () => void;
  loadModels: (endpoint: string) => Promise<void>;
  setModel: (modelId: string) => void;
  /** Stop the in-flight turn for `sessionId` (default: the active session). */
  cancelStream: (sessionId?: string) => Promise<void>;
  respondPermission: (
    allow: boolean,
    opts?: { grantFullAccess?: boolean },
  ) => Promise<void>;
  addLocalAssistantMessage: (content: string) => void;
  /** Steer the in-flight run: the message reaches the model at its next round
   *  boundary instead of waiting out the whole turn. Shows immediately as
   *  pending; `steer_applied` confirms it actually landed. */
  steerRun: (content: string) => Promise<void>;
  clearVisibleConversation: () => void;
  updateActiveSessionModel: (modelId: string) => Promise<void>;
  updateActiveSessionModelConfig: (config: {
    endpointId: string;
    modelId: string;
    policy: "fixed" | "prefer" | "auto";
  }) => Promise<void>;
  updateActiveSessionPermissionMode: (mode: PermissionMode) => Promise<void>;
  updateActiveSessionReasoningEffort: (effort: ReasoningEffort | null) => Promise<void>;
  exitAnonymous: () => void;

  /** Per-session stream listeners — kept alive across session switches so a
   *  background session keeps streaming into its own runtime bucket. */
  _unlisten: Record<string, UnlistenFn | undefined>;
  _unlistenSessionUpdated: Record<string, UnlistenFn | undefined>;
  _streamingMsgId: Record<string, string | undefined>;
  _draftMaterialization: Promise<Session> | null;
  _selectionRequestId: number;
  _modelsRequestId: number;
}

/** Selector: the active session's runtime slice (or the empty default). Use as
 *  `useChatStore(activeRuntime)`. */
export function activeRuntime(s: ChatStore): SessionRuntime {
  const id = s.activeSession?.id;
  return (id ? s.runtime[id] : undefined) ?? EMPTY_RUNTIME;
}

/** Resolve a session object by id across the active session and the list. */
function findSession(s: ChatStore, id: string): Session | undefined {
  if (s.activeSession?.id === id) return s.activeSession;
  return s.sessions.find((x) => x.id === id);
}

function isChatRunBusyError(error: unknown): boolean {
  return String(error).includes("CHAT_RUN_BUSY");
}

/** Selector: the id of whatever conversation is currently open — a draft or a
 *  real session, never both (every transition sets one and clears the other).
 *
 *  This is the single source of truth for "what is the workspace showing". The
 *  app shell used to keep its own copy in React state, which could drift from
 *  the store: the chat pane rendered one session while every session-scoped
 *  feature (tasks, git, interjections) addressed another. Derive, don't copy. */
export function openSessionId(s: ChatStore): string | null {
  return s.draftSession?.id ?? s.activeSession?.id ?? null;
}

export const useChatStore = create<ChatStore>((set, get) => ({
  sessions: [],
  activeSession: null,
  draftSession: null,
  runtime: {},
  models: [],
  activeModel: "anthropic/claude-opus-4-7",
  _unlisten: {},
  _unlistenSessionUpdated: {},
  _streamingMsgId: {},
  _draftMaterialization: null,
  _selectionRequestId: 0,
  _modelsRequestId: 0,

  beginDraft: ({ cwd = null, anonymous = false } = {}) => {
    const draft: DraftSession = {
      id: crypto.randomUUID(),
      cwd,
      anonymous,
      modelId: get().activeModel,
      permissionMode: "standard",
      text: "",
    };
    // Bumping the selection id cancels any in-flight session hydration, so a
    // slow round-trip can never land history on top of this new blank page.
    set((state) => ({
      activeSession: null,
      draftSession: draft,
      _selectionRequestId: state._selectionRequestId + 1,
    }));
    return draft;
  },

  setDraftProject: (cwd) => set((state) => ({
    draftSession: state.draftSession ? { ...state.draftSession, cwd } : null,
  })),

  setDraftAnonymous: (anonymous) => set((state) => ({
    draftSession: state.draftSession ? { ...state.draftSession, anonymous } : null,
  })),

  updateDraftText: (text) => set((state) => ({
    draftSession: state.draftSession ? { ...state.draftSession, text } : null,
  })),

  discardDraft: () => set({ draftSession: null }),

  loadSessions: async () => {
    const sessions = await invoke<Session[]>("list_sessions");
    set({ sessions });
    return sessions;
  },

  createSession: async (cwd, model) => {
    const title = cwd.split(/[/\\]/).pop() ?? "New Session";
    const session = await invoke<Session>("create_session", { title, cwd, modelId: model });
    set((s) => ({
      sessions: [session, ...s.sessions],
      activeSession: session,
      draftSession: null,
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

    const requestId = get()._selectionRequestId + 1;
    set({ _selectionRequestId: requestId });
    let session: Session;
    try {
      session = await invoke<Session>("get_session", { sessionId: id });
    } catch (error) {
      // The row is gone (deleted elsewhere, or an id that never existed).
      // Land on a blank draft rather than leaving the workspace pointed at a
      // session that isn't there — the previous behaviour was an unhandled
      // rejection that silently kept the *old* conversation on screen.
      if (get()._selectionRequestId !== requestId) return;
      console.error(`selectSession: ${id} could not be opened`, error);
      get().beginDraft();
      return;
    }
    if (get()._selectionRequestId !== requestId) return;

    // A frontend bucket can remain stuck at streaming=true after the backend
    // has already persisted its final reply. Ask the runtime authority before
    // deciding whether re-selection should preserve a genuinely live buffer or
    // recover from the persisted tail.
    let staleStreamingRevision: number | null = null;
    const selectedRuntime = get().runtime[id];
    if (selectedRuntime?.streaming) {
      const backendRunning = await invoke<boolean>("is_chat_running", {
        sessionId: id,
      });
      if (get()._selectionRequestId !== requestId) return;
      if (backendRunning) {
        set({ activeSession: session, activeModel: session.model_id });
        return;
      }
      staleStreamingRevision = selectedRuntime.revision;
    }

    // Load a fresh snapshot for an idle session or a stale streaming bucket.
    const page = await invoke<MessagePage>("get_message_page", {
      sessionId: id,
      beforeRowid: null,
      userTurnLimit: 8,
    });
    if (get()._selectionRequestId !== requestId) return;
    let recoveredStaleStream = false;
    set((s) => {
      const currentRuntime = s.runtime[id];
      // A background event may have started this session while its page was
      // crossing the bridge. Never replace a newly-live bucket with hydration.
      if (
        currentRuntime?.streaming &&
        (
          staleStreamingRevision == null ||
          currentRuntime.revision !== staleStreamingRevision
        )
      ) {
        return {
          activeSession: session,
          draftSession: null,
          activeModel: session.model_id,
        };
      }
      const localMessages =
        staleStreamingRevision != null ? currentRuntime?.localMessages ?? [] : [];
      const hydrated = freshRuntime(
        [
          ...dbMessagesToUI(
            page.messages,
            page.plans ?? [],
            page.turn_states ?? [],
          ),
          ...localMessages,
        ],
        {
          persistedMessages: page.messages,
          persistedPlans: page.plans ?? [],
          historyBeforeRowid: page.next_before_rowid ?? null,
          hasOlderHistory: page.has_more,
          historyTruncated: page.truncated ?? false,
        },
      );
      if (staleStreamingRevision != null && currentRuntime) {
        hydrated.queue = currentRuntime.queue;
        hydrated.localMessages = localMessages;
        recoveredStaleStream = currentRuntime.streaming;
      }
      const streamingMsgIds = { ...s._streamingMsgId };
      if (recoveredStaleStream) delete streamingMsgIds[id];
      return {
        activeSession: session,
        draftSession: null,
        activeModel: session.model_id,
        runtime: {
          ...s.runtime,
          [id]: hydrated,
        },
        _streamingMsgId: streamingMsgIds,
      };
    });
    if (recoveredStaleStream) {
      drainNextQueuedMessage(id, set, get);
    }
  },

  loadOlderMessages: async () => {
    const id = get().activeSession?.id;
    if (!id) return;
    const current = get().runtime[id];
    if (
      !current ||
      current.streaming ||
      current.loadingOlderHistory ||
      !current.hasOlderHistory ||
      current.historyBeforeRowid == null
    ) {
      return;
    }

    const expectedRevision = current.revision;
    const requestId = current.historyRequestId + 1;
    const beforeRowid = current.historyBeforeRowid;
    set((s) => {
      const runtime = s.runtime[id];
      if (!runtime || runtime.revision !== expectedRevision) return {};
      return {
        runtime: {
          ...s.runtime,
          [id]: {
            ...runtime,
            loadingOlderHistory: true,
            historyRequestId: requestId,
          },
        },
      };
    });

    try {
      const [page, latestPage] = await Promise.all([
        invoke<MessagePage>("get_message_page", {
          sessionId: id,
          beforeRowid,
          userTurnLimit: 8,
        }),
        // The user may have completed a live turn after this history bucket
        // was first hydrated. Refresh the persisted tail only on this explicit
        // history action; the revision guard below prevents a late response
        // from overwriting a newly-started stream.
        invoke<MessagePage>("get_message_page", {
          sessionId: id,
          beforeRowid: null,
          userTurnLimit: 8,
        }),
      ]);
      set((s) => {
        const runtime = s.runtime[id];
        if (!runtime || runtime.historyRequestId !== requestId) return {};
        if (
          runtime.streaming ||
          runtime.revision !== expectedRevision ||
          s.activeSession?.id !== id
        ) {
          return {
            runtime: {
              ...s.runtime,
              [id]: { ...runtime, loadingOlderHistory: false },
            },
          };
        }
        const persistedMessages = mergePersistedMessages(
          page.messages,
          mergePersistedMessages(runtime.persistedMessages, latestPage.messages),
        );
        const persistedPlans = mergePersistedPlans(
          page.plans ?? [],
          mergePersistedPlans(runtime.persistedPlans, latestPage.plans ?? []),
        );
        const turnStates = mergeTurnActivityStates(
          page.turn_states ?? [],
          latestPage.turn_states ?? [],
        );
        return {
          runtime: {
            ...s.runtime,
            [id]: {
              ...runtime,
              messages: [
                ...dbMessagesToUI(
                  persistedMessages,
                  persistedPlans,
                  turnStates,
                ),
                ...runtime.localMessages,
              ],
              persistedMessages,
              persistedPlans,
              historyBeforeRowid: page.next_before_rowid ?? null,
              hasOlderHistory: page.has_more,
              loadingOlderHistory: false,
              historyTruncated:
                runtime.historyTruncated ||
                (page.truncated ?? false) ||
                (latestPage.truncated ?? false),
              revision: runtime.revision + 1,
            },
          },
        };
      });
    } catch {
      set((s) => {
        const runtime = s.runtime[id];
        if (!runtime || runtime.historyRequestId !== requestId) return {};
        return {
          runtime: {
            ...s.runtime,
            [id]: { ...runtime, loadingOlderHistory: false },
          },
        };
      });
    }
  },

  deleteSession: async (id) => {
    const deleted = findSession(get(), id);
    const wasActive = get().activeSession?.id === id;
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
        runtime,
        _unlisten,
        _unlistenSessionUpdated,
        _streamingMsgId,
        ...(s.activeSession?.id === id ? { activeSession: null } : {}),
      };
    });
    // The workspace shows whatever the store says is open, so deleting the
    // conversation you are *in* has to leave something behind — otherwise the
    // shell has nothing to render. Land on a blank one, scoped to the same
    // project so the user stays where they were working.
    if (wasActive) {
      get().beginDraft({ cwd: deleted?.kind === "quick" ? null : deleted?.cwd ?? null });
    }
  },

  renameSession: async (id, title) => {
    const session = await invoke<Session>("update_session_title", { sessionId: id, title });
    set((s) => ({
      sessions: s.sessions.map((existing) => (existing.id === id ? session : existing)),
      ...(s.activeSession?.id === id
        ? { activeSession: session, activeModel: session.model_id }
        : {}),
    }));
  },

  sendMessage: async (content, sessionId, rootTurnId) => {
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

    // Persisted turns use the client-generated UUID as the exact durable root
    // identity. The backend either inserts this id in the admission
    // transaction or verifies an already-materialized draft row byte-for-byte.
    // Busy/retry paths preserve the same id, so no user input is duplicated or
    // orphaned behind a different SQLite root.
    const exactRootTurnId = rootTurnId ?? crypto.randomUUID();
    const userMsg: UIMessage = {
      id: exactRootTurnId,
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
          [id]: {
            ...prev,
            messages: [...prev.messages, userMsg, assistantMsg],
            streaming: true,
            transportDoneSucceeded: false,
            revision: prev.revision + 1,
          },
        },
        _streamingMsgId: { ...s._streamingMsgId, [id]: assistantMsgId },
      };
    });

    const unlisten = await onStream(id, (event: StreamEvent) => {
      // Read the open bubble per event rather than closing over it: a steer
      // splits the turn mid-flight, and a captured id would keep feeding the
      // bubble that was closed at the split.
      handleStreamEvent(event, id, get()._streamingMsgId[id] ?? assistantMsgId, set, get);
    });
    set((s) => ({ _unlisten: { ...s._unlisten, [id]: unlisten } }));

    try {
      if (isAnon) {
        await sendMessageAnonymous(
          id,
          content,
          anonHistory,
          target.cwd,
          target.model_id,
          target.endpoint_id,
          target.model_policy,
        );
      } else {
        await invoke("send_message", {
          sessionId: id,
          content,
          rootTurnId: exactRootTurnId,
        });
      }
    } catch (e) {
      if (isChatRunBusyError(e)) {
        set((s) => {
          const prev = s.runtime[id];
          if (!prev) return {};
          const _streamingMsgId = { ...s._streamingMsgId };
          if (_streamingMsgId[id] === assistantMsgId) {
            delete _streamingMsgId[id];
          }
          return {
            runtime: {
              ...s.runtime,
              [id]: {
                ...prev,
                messages: prev.messages.filter(
                  (message) => message.id !== userMsg.id && message.id !== assistantMsgId,
                ),
                localMessages: prev.localMessages.filter(
                  (message) => message.id !== userMsg.id && message.id !== assistantMsgId,
                ),
                // The optimistic bubbles were rejected, but CHAT_RUN_BUSY is
                // positive evidence that another run still owns the session.
                // Its turn_settled event remains the only terminal authority.
                streaming: true,
                transportDoneSucceeded: false,
                queue: [
                  {
                    id: userMsg.id,
                    content,
                    enqueuedAt: userMsg.createdAt,
                    rootTurnId: exactRootTurnId,
                  },
                  ...prev.queue,
                ],
                revision: prev.revision + 1,
              },
            },
            _streamingMsgId,
          };
        });
        return;
      }
      const errorPresentation = presentChatInvocationError(e);
      set((s) => {
        const prev = s.runtime[id];
        if (!prev) return {};
        return {
          runtime: {
            ...s.runtime,
            [id]: {
              ...prev,
              messages: prev.messages.map((m) =>
                m.id === assistantMsgId ? { ...m, ...errorPresentation } : m,
              ),
              transportDoneSucceeded: false,
              revision: prev.revision + 1,
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
    const draft = get().draftSession;
    if (!active && draft) {
      get().updateDraftText(text);
      // Anonymous drafts never reach the database: they become an in-memory
      // session instead, keeping the same "nothing exists until you send" rule
      // as every other draft.
      if (draft.anonymous) {
        const anon = materializeAnonymousDraft(draft, set, get);
        await get().sendMessage(text, anon.id);
        return "sent";
      }
      let materialization = get()._draftMaterialization;
      const firstMessageId = draft.firstMessageId ?? crypto.randomUUID();
      if (!draft.firstMessageId) {
        set((state) => ({
          draftSession:
            state.draftSession?.id === draft.id
              ? { ...state.draftSession, firstMessageId }
              : state.draftSession,
        }));
      }
      if (!materialization) {
        materialization = invoke<Session>("materialize_draft_session", {
          draftId: draft.id,
          cwd: draft.cwd,
          modelId: draft.modelId,
          firstMessageId,
          firstMessage: text,
        });
        set({ _draftMaterialization: materialization });
      }
      try {
        const session = await materialization;
        // A concurrent Enter joins the same materialization and must not start
        // a duplicate turn after the first caller has already begun streaming.
        const alreadyMaterialized = get().activeSession?.id === session.id;
        set((state) => ({
          activeSession: session,
          draftSession: null,
          activeModel: session.model_id,
          _draftMaterialization: null,
          runtime: {
            ...state.runtime,
            [session.id]: state.runtime[session.id] ?? freshRuntime(),
          },
          sessions: [session, ...state.sessions.filter((item) => item.id !== session.id)],
        }));
        if (!alreadyMaterialized) {
          await get().sendMessage(text, session.id, firstMessageId);
        }
        return "sent";
      } catch {
        set({ _draftMaterialization: null });
        return "failed";
      }
    }
    if (!active) return "sent";
    const id = active.id;
    const rt = get().runtime[id];
    if (!rt || !rt.streaming) {
      await get().sendMessage(text, id, crypto.randomUUID());
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
              {
                id: crypto.randomUUID(),
                content: text,
                enqueuedAt: Date.now(),
                rootTurnId: crypto.randomUUID(),
              },
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
    const requestId = get()._modelsRequestId + 1;
    set({ _modelsRequestId: requestId, models: [] });
    try {
      const models = await invoke<ModelInfo[]>("list_models", { endpointName: endpoint });
      if (get()._modelsRequestId !== requestId) return;
      set({ models });
    } catch {
      if (get()._modelsRequestId !== requestId) return;
      // silently ignore — user hasn't set key yet
    }
  },

  setModel: (modelId) => set({ activeModel: modelId }),

  updateActiveSessionModel: async (modelId) => {
    const activeSession = get().activeSession;
    const draftSession = get().draftSession;
    set({
      activeModel: modelId,
      draftSession: draftSession ? { ...draftSession, modelId } : draftSession,
    });
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

  updateActiveSessionModelConfig: async ({ endpointId, modelId, policy }) => {
    const activeSession = get().activeSession;
    set({
      activeModel: modelId,
      draftSession: get().draftSession
        ? { ...get().draftSession!, modelId }
        : get().draftSession,
    });
    if (!activeSession) return;
    if (activeSession.kind === "anonymous") {
      set({
        activeSession: {
          ...activeSession,
          endpoint_id: endpointId,
          model_id: modelId,
          model_policy: policy,
        },
      });
      return;
    }
    const session = await invoke<Session>("update_session_model_config", {
      sessionId: activeSession.id,
      endpointId,
      modelId,
      policy,
    });
    set((state) => ({
      activeSession: session,
      sessions: state.sessions.map((existing) =>
        existing.id === session.id ? session : existing
      ),
    }));
  },

  updateActiveSessionPermissionMode: async (mode) => {
    const session = get().activeSession;
    if (!session) return;
    const updated = await invoke<Session>("update_session_permission_mode", {
      sessionId: session.id,
      mode,
    });
    set((state) => ({
      activeSession: updated,
      sessions: state.sessions.map((item) => item.id === updated.id ? updated : item),
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

  cancelStream: async (sessionId) => {
    const id = sessionId ?? get().activeSession?.id;
    if (!id) return;
    // Tell the backend to stop the in-flight turn — otherwise the agent keeps
    // looping (burning tokens) after the UI already says "stopped". Cooperative:
    // it stops between rounds, never mid tool-call. Scoped to THIS chat session
    // only; it never affects the task scheduler / long task runs.
    // Keep the listener, permission, and streaming state until the backend
    // emits turn_settled. Transport-level done/error is not durable settlement.
    try {
      await invoke("cancel_chat", { sessionId: id });
    } catch {
      const detail =
        "停止请求未送达；当前运行仍在继续，系统已保留原运行状态。请稍后再次停止。";
      set((s) => {
        const prev = s.runtime[id];
        if (!prev) return {};
        const currentAssistantId =
          s._streamingMsgId[id] ??
          [...prev.messages].reverse().find((message) => message.role === "assistant")?.id;
        if (!currentAssistantId) return {};
        return {
          runtime: {
            ...s.runtime,
            [id]: {
              ...prev,
              messages: prev.messages.map((message) =>
                message.id === currentAssistantId
                  ? {
                      ...message,
                      content: message.content
                        ? `${message.content}\n\n${detail}`
                        : detail,
                      runtimeError: {
                        code: "CHAT_STOP_FAILED",
                        endpointId: null,
                        recoverable: true,
                      },
                    }
                  : message,
              ),
              revision: prev.revision + 1,
            },
          },
        };
      });
    }
  },

  respondPermission: async (allow, opts) => {
    const id = get().activeSession?.id;
    if (!id) return;
    const pending = get().runtime[id]?.pendingPermission;
    if (!pending) return;
    // “信任本会话并允许”：只把当前会话切到 trusted。权限不再是全局设置，
    // 也不要求用户维护工具 allow/ask/deny 明细；运行中的 agent 每次工具
    // 判断都会重读 sessions.permission_mode。
    if (allow && opts?.grantFullAccess) {
      try {
        await get().updateActiveSessionPermissionMode("trusted");
      } catch (e) {
        // Persisting failed — fall back to allow-once so the call doesn't hang.
        console.error("Failed to persist trusted permission mode for session:", e);
      }
    }
    await invoke("respond_to_permission", { intentId: pending.intentId, allow });
    set((s) => {
      const prev = s.runtime[id];
      if (!prev) return {};
      return {
        runtime: {
          ...s.runtime,
          [id]: {
            ...prev,
            ...markPermissionResponse(prev, pending.toolCallId, allow),
            revision: prev.revision + 1,
          },
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
      return {
        runtime: {
          ...s.runtime,
          [id]: {
            ...prev,
            messages: [...prev.messages, msg],
            localMessages: [...prev.localMessages, msg],
            revision: prev.revision + 1,
          },
        },
      };
    });
  },

  steerRun: async (content) => {
    const text = content.trim();
    const id = get().activeSession?.id;
    if (!text || !id) return;
    // Only a streaming chat turn confirms delivery (`steer_applied`) and only
    // it can recover an undelivered steer at its terminal state. When the
    // interjection is bound for the task scheduler instead, an optimistic
    // bubble would hang in "pending" forever with nothing to resolve it — the
    // input's own confirmation is the honest feedback there.
    if (!get().runtime[id]?.streaming) {
      await invoke("queue_interjection", { sessionId: id, message: text });
      return;
    }
    const msg: UIMessage = {
      id: crypto.randomUUID(),
      role: "user",
      content: text,
      createdAt: Date.now(),
      steerPending: true,
    };
    set((s) => {
      const prev = s.runtime[id] ?? freshRuntime();
      return {
        runtime: {
          ...s.runtime,
          [id]: {
            ...prev,
            messages: [...prev.messages, msg],
            localMessages: [...prev.localMessages, msg],
            revision: prev.revision + 1,
          },
        },
      };
    });
    // Position it the moment it is said, not when the loop gets to it. The
    // wait for a round boundary can run for minutes, and without splitting now
    // the agent's ongoing output keeps piling ABOVE the bubble — which is
    // exactly the reported symptom: 引导气泡一直在最下边.
    splitAssistantTurnAfterSteer(id, set);
    try {
      await invoke("queue_interjection", { sessionId: id, message: text });
    } catch (error) {
      // Never leave a bubble claiming to be on its way when it isn't — and
      // take the empty bubble the split opened for it with it.
      set((s) => {
        const prev = s.runtime[id];
        if (!prev) return {};
        return {
          runtime: {
            ...s.runtime,
            [id]: {
              ...prev,
              messages: dropEmptyTail(prev.messages.filter((m) => m.id !== msg.id)),
              localMessages: prev.localMessages.filter((m) => m.id !== msg.id),
              revision: prev.revision + 1,
            },
          },
        };
      });
      throw error;
    }
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
            persistedMessages: [],
            historyBeforeRowid: null,
            hasOlderHistory: false,
            loadingOlderHistory: false,
            historyTruncated: false,
            revision: prev.revision + 1,
            historyRequestId: prev.historyRequestId + 1,
            localMessages: [],
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

/** Turn an anonymous draft into the purely in-memory session it sends through:
 *  a client-generated id, kind "anonymous", and the draft's project directory
 *  (empty → the backend resolves a scratch dir). Never written to the DB, never
 *  listed. Anonymity is a property of the draft, not a separate kind of task —
 *  an anonymous conversation can still be scoped to a project. */
function materializeAnonymousDraft(
  draft: DraftSession,
  set: (partial: Partial<ChatStore>) => void,
  get: () => ChatStore,
): Session {
  const anon: Session = {
    id: draft.id,
    title: "匿名会话",
    cwd: draft.cwd ?? "",
    model_id: draft.modelId,
    endpoint_id: useSettingsStore.getState().settings?.default_endpoint ?? "openrouter",
    model_policy: useSettingsStore.getState().settings?.default_model_policy ?? "prefer",
    created_at: Date.now(),
    updated_at: Date.now(),
    total_input_tokens: 0,
    total_output_tokens: 0,
    kind: "anonymous",
  };
  set({
    activeSession: anon,
    draftSession: null,
    runtime: { ...get().runtime, [anon.id]: freshRuntime() },
  });
  return anon;
}

/// Re-send steers the loop never reached. Returns true when one was recovered,
/// so the caller defers the queue drain — the recovered steer becomes the next
/// turn and anything queued still follows it in order.
function recoverUndeliveredSteers(
  sessionId: string,
  set: (fn: (s: ChatStore) => Partial<ChatStore>) => void,
  get: () => ChatStore,
): boolean {
  const runtime = get().runtime[sessionId];
  const undelivered = runtime?.messages.filter((m) => m.steerPending) ?? [];
  if (undelivered.length === 0) return false;

  const ids = new Set(undelivered.map((m) => m.id));
  set((s) => {
    const prev = s.runtime[sessionId];
    if (!prev) return {};
    return {
      runtime: {
        ...s.runtime,
        [sessionId]: {
          ...prev,
          messages: dropEmptyTail(prev.messages.filter((m) => !ids.has(m.id))),
          localMessages: prev.localMessages.filter((m) => !ids.has(m.id)),
          revision: prev.revision + 1,
        },
      },
    };
  });

  // Oldest first: the first becomes the next turn, the rest queue behind it so
  // several rapid steers keep their order.
  const [first, ...rest] = undelivered;
  if (rest.length > 0) {
    set((s) => {
      const prev = s.runtime[sessionId];
      if (!prev) return {};
      return {
        runtime: {
          ...s.runtime,
          [sessionId]: {
            ...prev,
            queue: [
              ...rest.map((m) => ({
                id: crypto.randomUUID(),
                content: m.content,
                enqueuedAt: m.createdAt,
              })),
              ...prev.queue,
            ],
          },
        },
      };
    });
  }
  setTimeout(() => {
    void get().sendMessage(first.content, sessionId);
  }, 0);
  return true;
}

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
    void get().sendMessage(
      next.content,
      sessionId,
      next.rootTurnId,
    );
  }, 0);
  return true;
}

/// Drop a trailing assistant bubble that never received anything. The steer
/// split opens one eagerly; if the steer is then withdrawn or never lands,
/// that placeholder must not survive as a blank turn.
function dropEmptyTail(messages: UIMessage[]): UIMessage[] {
  const tail = messages[messages.length - 1];
  const empty =
    tail?.role === "assistant" &&
    !tail.content &&
    !(tail.toolCalls?.length ?? 0) &&
    !(tail.segments?.length ?? 0);
  return empty ? messages.slice(0, -1) : messages;
}

/// Freeze the assistant bubble that was streaming when the user spoke, and
/// start a new one below their message, so the turn reads in the order it
/// happened: work so far → what you said → what it did next.
///
/// Live-only. Hydrated history already interleaves correctly because the
/// persisted rows carry the real ordering — this gap existed only on screen.
function splitAssistantTurnAfterSteer(
  sessionId: string,
  set: (fn: (s: ChatStore) => Partial<ChatStore>) => void,
) {
  set((s) => {
    const prev = s.runtime[sessionId];
    const openId = s._streamingMsgId[sessionId];
    if (!prev || !openId) return {};
    const openIndex = prev.messages.findIndex((m) => m.id === openId);
    // Nothing streamed yet this round: the empty bubble is still below the
    // steer's insertion point, so there is nothing to split.
    if (openIndex < 0) return {};

    const now = Date.now();
    const messages = prev.messages.slice();
    const settled = messages[openIndex];
    messages[openIndex] = {
      ...settled,
      durationMs: settled.durationMs ?? Math.max(0, now - settled.createdAt),
    };
    const resumed: UIMessage = {
      id: crypto.randomUUID(),
      role: "assistant",
      content: "",
      createdAt: now,
    };
    messages.push(resumed);

    return {
      runtime: {
        ...s.runtime,
        [sessionId]: { ...prev, messages, revision: prev.revision + 1 },
      },
      _streamingMsgId: { ...s._streamingMsgId, [sessionId]: resumed.id },
    };
  });
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
    return {
      runtime: {
        ...s.runtime,
        [sessionId]: {
          ...prev,
          ...reduced,
          revision: prev.revision + 1,
        },
      },
    };
  });


  // Queue drain is settlement-driven. Transport done/error can be followed by
  // durable recovery, so neither event is permission to start another run.
  const nowStreaming = get().runtime[sessionId]?.streaming ?? false;
  if (wasStreaming && !nowStreaming) {
    // A steer typed after the loop's last round boundary was never delivered.
    // The backend drops it at turn cleanup so it can't leak into an unrelated
    // later turn, which leaves us to re-send it as an ordinary message —
    // otherwise the user watches their words vanish.
    if (recoverUndeliveredSteers(sessionId, set, get)) {
      return; // it becomes the next turn — defer the queue drain and post-mortem
    }
    if (drainNextQueuedMessage(sessionId, set, get)) {
      return; // more conversation coming — defer post-mortem
    }

    // Chat-end self-evolution trigger. Mirrors the task path (stores/tasks.ts).
    // Guards: throttled per session, skip too-short conversations, require a
    // successful transport-level done before completed settlement, and NEVER
    // run for anonymous chats (no-trace = no learning).
    const session = findSession(get(), sessionId);
    if (
      session &&
      session.kind !== "anonymous" &&
      event.type === "turn_settled" &&
      event.status === "completed" &&
      get().runtime[sessionId]?.transportDoneSucceeded === true &&
      (get().runtime[sessionId]?.messages.length ?? 0) >= POSTMORTEM_MIN_MESSAGES
    ) {
      const last = _lastPostmortemAt[session.id] ?? 0;
      if (Date.now() - last >= POSTMORTEM_THROTTLE_MS) {
        _lastPostmortemAt[session.id] = Date.now();
        // Local deterministic cross-session mining runs by DEFAULT: it makes no
        // model call and nothing leaves the machine, so the self-evolution loop
        // produces evidence-backed candidates out of the box. Fire-and-forget.
        invoke("mine_cross_session_patterns", { cwd: session.cwd }).catch((e) => {
          // eslint-disable-next-line no-console
          console.warn("local pattern mining failed (non-fatal)", e);
        });
        // The model-based post-mortem sends a redacted summary to the configured
        // provider, so it stays strictly opt-in (privacy + token cost).
        if (useSettingsStore.getState().settings?.remote_postmortem_enabled === true) {
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
}

function mergePersistedMessages(
  older: Message[],
  newer: Message[],
): Message[] {
  const seen = new Set<string>();
  const merged: Message[] = [];
  for (const message of [...older, ...newer]) {
    if (seen.has(message.id)) continue;
    seen.add(message.id);
    merged.push(message);
  }
  return merged;
}

function mergePersistedPlans(
  older: TurnPlanSnapshot[],
  newer: TurnPlanSnapshot[],
): TurnPlanSnapshot[] {
  const latest = new Map<string, TurnPlanSnapshot>();
  for (const plan of [...older, ...newer]) {
    const existing = latest.get(plan.root_turn_id);
    if (!existing || existing.revision < plan.revision) {
      latest.set(plan.root_turn_id, plan);
    }
  }
  return [...latest.values()].sort((a, b) => a.created_at - b.created_at);
}

function mergeTurnActivityStates(
  older: TurnActivitySnapshot[],
  newer: TurnActivitySnapshot[],
): TurnActivitySnapshot[] {
  const latest = new Map<string, TurnActivitySnapshot>();
  for (const state of [...older, ...newer]) {
    const existing = latest.get(state.root_turn_id);
    if (!existing || existing.revision < state.revision) {
      latest.set(state.root_turn_id, state);
    }
  }
  return [...latest.values()].sort((a, b) => a.updated_at - b.updated_at);
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
      if (call.function.name === "update_plan") return [];
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
  status: "done" | "blocked" | "error" | "denied" | "cancelled";
} | null {
  try {
    const replay = JSON.parse(raw) as PersistedToolReplay;
    if (typeof replay.tool_call_id !== "string" || typeof replay.content !== "string") {
      return null;
    }
    const status =
      replay.status === "error" ||
      replay.status === "blocked" ||
      replay.status === "denied" ||
      replay.status === "cancelled" ||
      replay.status === "done"
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
export function dbMessagesToUI(
  messages: Message[],
  plans: TurnPlanSnapshot[] = [],
  turnStates: TurnActivitySnapshot[] = [],
): UIMessage[] {
  const hydrated: UIMessage[] = [];
  const toolOwners = new Map<string, number>();

  for (const message of messages) {
    const completionState = message.completion_state;
    if (completionState === "gate_warning") {
      const answer = [...hydrated].reverse().find((item) => item.role === "assistant");
      if (answer) {
        answer.gateActions = [
          ...(answer.gateActions ?? []),
          { kind: "warning", detail: message.content },
        ];
      }
      continue;
    }
    // Gate prompts are framework instructions the loop injects as role=user.
    // They are neither the user's words nor an answer, so they never enter the
    // transcript — but they also never delete anything from it. The work each
    // recovery round produced stays visible as ordinary timeline steps.
    if (
      completionState === "gate_recovery" ||
      completionState === "gate_ready" ||
      completionState === "gate_blocked"
    ) {
      continue;
    }
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

  for (const plan of plans) {
    const rootIndex = hydrated.findIndex(
      (message) => message.id === plan.root_turn_id && message.role === "user",
    );
    if (rootIndex < 0) continue;
    let nextRootIndex = hydrated.length;
    for (let index = rootIndex + 1; index < hydrated.length; index += 1) {
      if (hydrated[index].role === "user") {
        nextRootIndex = index;
        break;
      }
    }
    const turnRows = hydrated.slice(rootIndex + 1, nextRootIndex);
    const finalAssistant = [...turnRows]
      .reverse()
      .find((message) => message.role === "assistant" && message.content.trim());
    if (!finalAssistant) continue;
    finalAssistant.plan = turnPlanFromEvent(plan);
    finalAssistant.durationMs = Math.max(
      0,
      finalAssistant.createdAt - hydrated[rootIndex].createdAt,
    );
    const turnTools = turnRows
      .flatMap((message) => message.toolCalls ?? [])
      .filter((tool) => tool.name !== "update_plan");
    finalAssistant.turnToolCallCount = turnTools.length;
    finalAssistant.turnToolCalls = turnTools.slice(-200);
  }

  for (const activity of turnStates) {
    const rootIndex = hydrated.findIndex(
      (message) =>
        message.id === activity.root_turn_id && message.role === "user",
    );
    if (rootIndex < 0) continue;
    let nextRootIndex = hydrated.length;
    for (let index = rootIndex + 1; index < hydrated.length; index += 1) {
      if (hydrated[index].role === "user") {
        nextRootIndex = index;
        break;
      }
    }
    const assistant = [...hydrated.slice(rootIndex + 1, nextRootIndex)]
      .reverse()
      .find((message) => message.role === "assistant");
    // A crash may happen before the first assistant row exists. Bind the
    // durable objective projection to the root user row instead of silently
    // dropping it during hydration.
    const activityTarget = assistant ?? hydrated[rootIndex];
    activityTarget.turnActivity = {
      rootTurnId: activity.root_turn_id,
      revision: activity.revision,
      phase: activity.phase,
      status: activity.status,
      kind: activity.recent_activity_kind,
      label: activity.recent_activity_label,
      waitingReason: activity.waiting_reason ?? null,
      updatedAt: activity.updated_at,
      terminalReason: activity.terminal_reason ?? null,
      objectiveId: activity.objective_id,
      objectiveStatus: activity.objective_status,
      recoveryOwner: activity.recovery_owner ?? null,
      nextObservationAt: activity.next_observation_at ?? null,
      lastProgressAt: activity.last_progress_at ?? null,
    };
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
    ...(m.completion_state ? { completionState: m.completion_state } : {}),
  };
}
