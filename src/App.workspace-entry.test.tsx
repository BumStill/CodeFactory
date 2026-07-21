// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import App from "./App";

const mocks = vi.hoisted(() => ({
  beginQuickDraft: vi.fn(() => ({ id: "draft-start", mode: "quick", cwd: null, modelId: "model", text: "" })),
  createSession: vi.fn(),
  invoke: vi.fn(),
}));

vi.mock("./pages/Home/HomePage", () => ({ HomePage: () => <main>旧首页不应出现</main> }));
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
vi.mock("./stores/chat", () => ({
  useChatStore: () => ({
    activeModel: "model",
    createSession: mocks.createSession,
    beginQuickDraft: mocks.beginQuickDraft,
  }),
}));
vi.mock("./stores/chatgptCatalog", () => ({ syncChatGptCatalog: vi.fn() }));
vi.mock("./lib/tauri", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

describe("App default workspace entry", () => {
  beforeEach(() => Object.values(mocks).forEach((mock) => mock.mockClear()));

  it("opens directly into the workspace with one in-memory draft and no backend session creation", async () => {
    render(<App />);

    const workspace = await screen.findByRole("main", { name: "会话工作区" });
    expect(workspace).toHaveAttribute("data-session-id", "draft-start");
    expect(screen.queryByText("旧首页不应出现")).not.toBeInTheDocument();
    expect(screen.queryByText("首次引导遮罩")).not.toBeInTheDocument();
    expect(mocks.beginQuickDraft).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(mocks.createSession).not.toHaveBeenCalled());
    expect(mocks.invoke).not.toHaveBeenCalledWith(
      expect.stringMatching(/create_session|create_quick_session|materialize_draft_session/),
      expect.anything(),
    );
  });
});
