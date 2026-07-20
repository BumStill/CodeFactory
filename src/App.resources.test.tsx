// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "./App";

vi.mock("./pages/Home/HomePage", () => ({
  HomePage: ({ onOpenResources }: { onOpenResources: () => void }) => (
    <button onClick={onOpenResources}>打开资源中心</button>
  ),
}));
vi.mock("./pages/Resources/ResourcesPage", () => ({
  ResourcesPage: () => <main aria-label="资源中心页面">资源中心已打开</main>,
}));
vi.mock("./pages/Workspace/WorkspacePage", () => ({ WorkspacePage: () => null }));
vi.mock("./pages/ControlPlane/ControlPlanePage", () => ({ ControlPlanePage: () => null }));
vi.mock("./pages/Benchmarks/BenchmarksPage", () => ({ BenchmarksPage: () => null }));
vi.mock("./pages/Evolution/EvolutionWorkbenchPage", () => ({ EvolutionWorkbenchPage: () => null }));
vi.mock("./pages/Settings/SettingsPage", () => ({ SettingsPage: () => null }));
vi.mock("./pages/Profile/ProfilePage", () => ({ ProfilePage: () => null }));
vi.mock("./components/Toast", () => ({ ToastContainer: () => null }));
vi.mock("./components/EvidenceViewer", () => ({ EvidenceViewer: () => null }));
vi.mock("./components/UpdaterBanner", () => ({ UpdaterBanner: () => null }));
vi.mock("./components/OnboardingOverlay", () => ({ OnboardingOverlay: () => null }));
vi.mock("./stores/settings", () => ({
  useSettingsStore: (selector: (state: { load: () => void; settings: { onboarded: boolean } }) => unknown) =>
    selector({ load: vi.fn(), settings: { onboarded: true } }),
}));
vi.mock("./stores/chat", () => ({
  useChatStore: () => ({ activeModel: "test-model", createSession: vi.fn() }),
}));
vi.mock("./stores/chatgptCatalog", () => ({ syncChatGptCatalog: vi.fn() }));
vi.mock("./lib/tauri", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

describe("App resource navigation", () => {
  it("routes the home backend entry to ResourcesPage", async () => {
    render(<App />);

    await userEvent.click(screen.getByRole("button", { name: "打开资源中心" }));

    expect(screen.getByRole("main", { name: "资源中心页面" })).toHaveTextContent("资源中心已打开");
  });
});
