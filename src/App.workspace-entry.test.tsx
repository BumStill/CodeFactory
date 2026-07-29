// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import App from "./App";
import { useChatStore } from "./stores/chat";

const mocks = vi.hoisted(() => ({
  beginDraft: vi.fn((_opts?: { cwd?: string | null }) => ({
    id: "draft-start", cwd: null, anonymous: false, modelId: "model", text: "",
  })),
  createSession: vi.fn(),
  selectSession: vi.fn(),
  loadSessions: vi.fn(async () => [] as Array<{ id: string }>),
  invoke: vi.fn(),
}));

vi.mock("./pages/Workspace/WorkspacePage", () => ({
  WorkspacePage: ({ sessionId }: { sessionId: string }) => (
    <main aria-label="会话工作区" data-session-id={sessionId}>左侧会话 + 右侧会话窗</main>
  ),
}));
vi.mock("./pages/Resources/ResourcesPage", () => ({ ResourcesPage: () => null }));
vi.mock("./pages/ControlPlane/ControlPlanePage", () => ({ ControlPlanePage: () => null }));
vi.mock("./pages/Benchmarks/BenchmarksPage", () => ({ BenchmarksPage: () => null }));
vi.mock("./pages/Evolution/EvolutionWorkbenchPage", () => ({ EvolutionWorkbenchPage: () => null }));
vi.mock("./pages/Settings/SettingsPage", () => ({ SettingsPage: () => null }));
vi.mock("./pages/Profile/ProfilePage", () => ({ ProfilePage: () => null }));
vi.mock("./components/Toast", () => ({ ToastContainer: () => null }));
vi.mock("./components/EvidenceViewer", () => ({ EvidenceViewer: () => null }));
vi.mock("./components/UpdaterBanner", () => ({ UpdaterBanner: () => null }));
vi.mock("./stores/settings", () => ({
  useSettingsStore: (selector: (state: { load: () => void; settings: { onboarded: boolean } }) => unknown) =>
    selector({ load: vi.fn(), settings: { onboarded: false } }),
}));
// The store is the single source of truth for which conversation is open, so
// the mock has to behave like one — a real (tiny) zustand store, so beginDraft
// actually re-renders the shell the way it does in the app.
interface FakeChatState {
  activeModel: string;
  sessions: Array<{ id: string }>;
  activeSession: { id: string } | null;
  draftSession: { id: string } | null;
  createSession: typeof mocks.createSession;
  loadSessions: () => Promise<Array<{ id: string }>>;
  selectSession: (id: string) => Promise<void>;
  beginDraft: (opts?: { cwd?: string | null }) => { id: string };
}
vi.mock("./stores/chat", async () => {
  const { create } = await import("zustand");
  const useChatStore = create<FakeChatState>((set) => ({
    activeModel: "model",
    sessions: [],
    activeSession: null,
    draftSession: null,
    createSession: mocks.createSession,
    loadSessions: async () => {
      const sessions = await mocks.loadSessions();
      set({ sessions });
      return sessions;
    },
    selectSession: async (id: string) => {
      await mocks.selectSession(id);
      set((state) => ({
        activeSession: state.sessions.find((session) => session.id === id) ?? { id },
        draftSession: null,
      }));
    },
    beginDraft: (opts) => {
      const draft = mocks.beginDraft(opts);
      set({ draftSession: draft, activeSession: null });
      return draft;
    },
  }));
  return {
    useChatStore,
    openSessionId: (s: FakeChatState) => s.draftSession?.id ?? s.activeSession?.id ?? null,
  };
});
vi.mock("./stores/chatgptCatalog", () => ({ syncChatGptCatalog: vi.fn() }));
vi.mock("./lib/tauri", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

describe("App default workspace entry", () => {
  beforeEach(() => {
    Object.values(mocks).forEach((mock) => mock.mockClear());
    mocks.loadSessions.mockResolvedValue([]);
    useChatStore.setState({ sessions: [], activeSession: null, draftSession: null });
  });

  it("opens the latest existing session on first launch", async () => {
    mocks.loadSessions.mockResolvedValue([{ id: "latest" }, { id: "older" }]);

    render(<App />);

    const workspace = await screen.findByRole("main", { name: "会话工作区" });
    expect(workspace).toHaveAttribute("data-session-id", "latest");
    expect(mocks.loadSessions).toHaveBeenCalledTimes(1);
    expect(mocks.selectSession).toHaveBeenCalledWith("latest");
    expect(mocks.beginDraft).not.toHaveBeenCalled();
  });

  it("opens directly into the workspace with one in-memory draft when there is no history", async () => {
    render(<App />);

    const workspace = await screen.findByRole("main", { name: "会话工作区" });
    expect(workspace).toHaveAttribute("data-session-id", "draft-start");
    expect(mocks.loadSessions).toHaveBeenCalledTimes(1);
    expect(mocks.beginDraft).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(mocks.createSession).not.toHaveBeenCalled());
    expect(mocks.invoke).not.toHaveBeenCalledWith(
      expect.stringMatching(/create_session|materialize_draft_session/),
      expect.anything(),
    );
  });
});
