// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "./App";

const draft = { id: "draft-start", mode: "quick", cwd: null, modelId: "test-model", text: "" };

vi.mock("./pages/Workspace/WorkspacePage", () => ({
  WorkspacePage: ({
    sessionId,
    initialTaskLogId,
    onOpenUsage,
  }: {
    sessionId: string;
    initialTaskLogId?: string | null;
    onOpenUsage?: () => void;
  }) => (
    <main aria-label="会话工作区" data-session-id={sessionId} data-task-id={initialTaskLogId ?? ""}>
      <button onClick={onOpenUsage}>打开用量</button>
    </main>
  ),
}));
vi.mock("./pages/Settings/SettingsPage", () => ({
  SettingsPage: ({
    initialTab,
    onOpenJobLog,
  }: {
    initialTab?: string;
    onOpenJobLog?: (sessionId: string, taskId: string) => void;
  }) => (
    <main aria-label="设置页" data-tab={initialTab ?? ""}>
      <button onClick={() => onOpenJobLog?.("project-session", "task-loop")}>查看作业日志</button>
    </main>
  ),
}));
vi.mock("./pages/Resources/ResourcesPage", () => ({ ResourcesPage: () => null }));
vi.mock("./pages/ControlPlane/ControlPlanePage", () => ({ ControlPlanePage: () => null }));
vi.mock("./pages/Benchmarks/BenchmarksPage", () => ({ BenchmarksPage: () => null }));
vi.mock("./pages/Evolution/EvolutionWorkbenchPage", () => ({ EvolutionWorkbenchPage: () => null }));
vi.mock("./pages/Profile/ProfilePage", () => ({ ProfilePage: () => null }));
vi.mock("./components/Toast", () => ({ ToastContainer: () => null }));
vi.mock("./components/EvidenceViewer", () => ({ EvidenceViewer: () => null }));
vi.mock("./components/UpdaterBanner", () => ({ UpdaterBanner: () => null }));
vi.mock("./stores/settings", () => ({
  useSettingsStore: (selector: (state: { load: () => void; settings: { onboarded: boolean } }) => unknown) =>
    selector({ load: vi.fn(), settings: { onboarded: true } }),
}));
vi.mock("./stores/chat", () => ({
  useChatStore: () => ({ draftSession: null, beginQuickDraft: vi.fn(() => draft) }),
}));
vi.mock("./stores/chatgptCatalog", () => ({ syncChatGptCatalog: vi.fn() }));

describe("App usage drill-down routing", () => {
  it("opens a real parent session and reveals the selected task log", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.click(await screen.findByRole("button", { name: "打开用量" }));
    expect(screen.getByRole("main", { name: "设置页" })).toHaveAttribute("data-tab", "usage");

    await user.click(screen.getByRole("button", { name: "查看作业日志" }));
    const workspace = screen.getByRole("main", { name: "会话工作区" });
    expect(workspace).toHaveAttribute("data-session-id", "project-session");
    expect(workspace).toHaveAttribute("data-task-id", "task-loop");
  });
});
