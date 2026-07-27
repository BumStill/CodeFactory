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
vi.mock("./components/OnboardingOverlay", () => ({ OnboardingOverlay: () => <div>首次引导遮罩</div> }));
vi.mock("./stores/settings", () => ({
  useSettingsStore: (selector: (state: { load: () => void; settings: { onboarded: boolean } }) => unknown) =>
    selector({ load: vi.fn(), settings: { onboarded: false } }),
}));
// The store is the single source of truth for which conversation is open, so
// the mock has to behave like one — a real (tiny) zustand store, so beginDraft
// actually re-renders the shell the way it does in the app.
interface FakeChatState {
  activeModel: string;
  activeSession: { id: string } | null;
  draftSession: { id: string } | null;
  createSession: typeof mocks.createSession;
  selectSession: typeof mocks.selectSession;
  beginDraft: (opts?: { cwd?: string | null }) => { id: string };
}
vi.mock("./stores/chat", async () => {
  const { create } = await import("zustand");
  const useChatStore = create<FakeChatState>((set) => ({
    activeModel: "model",
    activeSession: null,
    draftSession: null,
    createSession: mocks.createSession,
    selectSession: mocks.selectSession,
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
    useChatStore.setState({ activeSession: null, draftSession: null });
  });

  it("opens directly into the workspace with one in-memory draft and no backend session creation", async () => {
    render(<App />);

    const workspace = await screen.findByRole("main", { name: "会话工作区" });
    expect(workspace).toHaveAttribute("data-session-id", "draft-start");
    expect(screen.queryByText("首次引导遮罩")).not.toBeInTheDocument();
    expect(mocks.beginDraft).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(mocks.createSession).not.toHaveBeenCalled());
    expect(mocks.invoke).not.toHaveBeenCalledWith(
      expect.stringMatching(/create_session|materialize_draft_session/),
      expect.anything(),
    );
  });
});
