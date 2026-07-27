// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "./App";


vi.mock("./pages/Resources/ResourcesPage", () => ({
  ResourcesPage: () => <main aria-label="资源中心页面">资源中心已打开</main>,
}));
vi.mock("./pages/Workspace/WorkspacePage", () => ({
  WorkspacePage: ({ onOpenSettings }: { onOpenSettings?: () => void }) => (
    <button onClick={onOpenSettings}>打开设置</button>
  ),
}));
vi.mock("./pages/ControlPlane/ControlPlanePage", () => ({ ControlPlanePage: () => null }));
vi.mock("./pages/Benchmarks/BenchmarksPage", () => ({ BenchmarksPage: () => null }));
vi.mock("./pages/Evolution/EvolutionWorkbenchPage", () => ({ EvolutionWorkbenchPage: () => null }));
vi.mock("./pages/Settings/SettingsPage", () => ({
  SettingsPage: ({ onOpenResources, initialTab }: { onOpenResources?: () => void; initialTab?: string }) => (
    <button data-tab={initialTab} onClick={onOpenResources}>从设置打开资源中心</button>
  ),
}));
vi.mock("./pages/Profile/ProfilePage", () => ({ ProfilePage: () => null }));
vi.mock("./components/Toast", () => ({ ToastContainer: () => null }));
vi.mock("./components/EvidenceViewer", () => ({ EvidenceViewer: () => null }));
vi.mock("./components/UpdaterBanner", () => ({ UpdaterBanner: () => null }));
vi.mock("./components/OnboardingOverlay", () => ({ OnboardingOverlay: () => null }));
vi.mock("./stores/settings", () => ({
  useSettingsStore: (selector: (state: { load: () => void; settings: { onboarded: boolean } }) => unknown) =>
    selector({ load: vi.fn(), settings: { onboarded: true } }),
}));
vi.mock("./stores/chat", async () => {
  const { createFakeChatModule } = await import("./test-utils/fakeChatStore");
  return createFakeChatModule();
});
vi.mock("./stores/chatgptCatalog", () => ({ syncChatGptCatalog: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

describe("App resource navigation", () => {
  it("routes the settings capability entry to ResourcesPage", async () => {
    render(<App />);

    await userEvent.click(await screen.findByRole("button", { name: "打开设置" }));
    const resourceEntry = await screen.findByRole("button", { name: "从设置打开资源中心" });
    expect(resourceEntry).toHaveAttribute("data-tab", "capabilities");
    await userEvent.click(resourceEntry);

    expect(screen.getByRole("main", { name: "资源中心页面" })).toHaveTextContent("资源中心已打开");
  });
});
