// SPDX-License-Identifier: Apache-2.0
//
// A minimal stand-in for the chat store, for App-shell tests that care about
// routing rather than chat behaviour.
//
// It is a REAL zustand store rather than a plain object because the shell now
// derives the open-conversation id from the store — a static mock would never
// re-render, and the resulting green test would be meaningless.
import type { StoreApi, UseBoundStore } from "zustand";

export interface FakeChatState {
  activeModel: string;
  activeSession: { id: string } | null;
  draftSession: { id: string; cwd: string | null } | null;
  beginDraft: (opts?: { cwd?: string | null; anonymous?: boolean }) => { id: string; cwd: string | null };
  selectSession: (id: string) => Promise<void>;
  createSession: (...args: unknown[]) => unknown;
}

export interface FakeChatModule {
  useChatStore: UseBoundStore<StoreApi<FakeChatState>>;
  openSessionId: (s: FakeChatState) => string | null;
}

/**
 * Build the module shape `vi.mock("./stores/chat", …)` must return.
 *
 * `onBeginDraft` / `onSelectSession` let a test observe the calls; the store
 * still performs the state transition the real one does, so the shell reacts
 * exactly as it would in the app.
 */
export async function createFakeChatModule(opts: {
  draftId?: string;
  onBeginDraft?: (opts?: { cwd?: string | null; anonymous?: boolean }) => void;
  onSelectSession?: (id: string) => void;
  createSession?: (...args: unknown[]) => unknown;
} = {}): Promise<FakeChatModule> {
  const { create } = await import("zustand");
  const useChatStore = create<FakeChatState>((set) => ({
    activeModel: "test-model",
    activeSession: null,
    draftSession: null,
    createSession: opts.createSession ?? (() => undefined),
    beginDraft: (draftOpts) => {
      opts.onBeginDraft?.(draftOpts);
      const draft = { id: opts.draftId ?? "draft-start", cwd: draftOpts?.cwd ?? null };
      set({ draftSession: draft, activeSession: null });
      return draft;
    },
    selectSession: async (id: string) => {
      opts.onSelectSession?.(id);
      set({ activeSession: { id }, draftSession: null });
    },
  }));
  return {
    useChatStore,
    openSessionId: (s: FakeChatState) => s.draftSession?.id ?? s.activeSession?.id ?? null,
  };
}
