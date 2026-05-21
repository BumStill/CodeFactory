// SPDX-License-Identifier: Apache-2.0
import { create } from "zustand";
import { invoke, onStream, onSessionUpdated } from "../lib/tauri";
import type { Message, Session, StreamEvent, ModelInfo } from "../lib/tauri";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  markPermissionResponse,
  reduceChatStreamEvent,
  type PendingPermission,
  type ToolCallState,
  type UIMessage,
} from "./chatEvents";

export type { PendingPermission, ToolCallState, UIMessage };

interface ChatStore {
  sessions: Session[];
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

  loadSessions: () => Promise<void>;
  createSession: (cwd: string, model: string) => Promise<Session>;
  selectSession: (id: string) => Promise<void>;
  deleteSession: (id: string) => Promise<void>;
  sendMessage: (content: string) => Promise<void>;
  loadModels: (endpoint: string) => Promise<void>;
  setModel: (modelId: string) => void;
  cancelStream: () => void;
  respondPermission: (allow: boolean) => Promise<void>;
  addLocalAssistantMessage: (content: string) => void;
  clearVisibleConversation: () => void;
  updateActiveSessionModel: (modelId: string) => Promise<void>;

  _unlisten?: UnlistenFn;
  _unlistenSessionUpdated?: UnlistenFn;
  _streamingMsgId?: string;
}

export const useChatStore = create<ChatStore>((set, get) => ({
  sessions: [],
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

  loadSessions: async () => {
    const sessions = await invoke<Session[]>("list_sessions");
    set({ sessions });
  },

  createSession: async (cwd, model) => {
    const title = cwd.split(/[/\\]/).pop() ?? "New Session";
    const session = await invoke<Session>("create_session", { title, cwd, modelId: model });
    set((s) => ({ sessions: [session, ...s.sessions], activeSession: session, messages: [] }));
    return session;
  },

  selectSession: async (id) => {
    const session = await invoke<Session>("get_session", { sessionId: id });
    const msgs = await invoke<Message[]>("get_messages", { sessionId: id });
    set({
      activeSession: session,
      messages: msgs.map(dbToUI),
      activeModel: session.model_id,
      inputTokenTotal: 0,
      outputTokenTotal: 0,
      pendingPermission: null,
      contextUsage: null,
      compressionToast: null,
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

    // Cancel any previous listeners
    _unlisten?.();
    _unlistenSessionUpdated?.();

    // Subscribe to session title update events before sending
    const unlistenSessionUpdated = await onSessionUpdated(activeSession.id, (session) => {
      set((s) => ({
        activeSession: s.activeSession?.id === session.id ? session : s.activeSession,
        sessions: s.sessions.map((existing) =>
          existing.id === session.id ? session : existing
        ),
      }));
    });
    set({ _unlistenSessionUpdated: unlistenSessionUpdated });

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
      await invoke("send_message", {
        sessionId: activeSession.id,
        content,
      });
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

  cancelStream: () => {
    get()._unlisten?.();
    set({ streaming: false, _unlisten: undefined, pendingPermission: null });
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

function handleStreamEvent(
  event: StreamEvent,
  msgId: string,
  set: (fn: (s: ChatStore) => Partial<ChatStore>) => void,
  _get: () => ChatStore
) {
  set((s) => reduceChatStreamEvent(s, event, msgId));
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
