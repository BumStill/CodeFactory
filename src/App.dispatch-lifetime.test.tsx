// SPDX-License-Identifier: Apache-2.0
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import App from "./App";

const mocks = vi.hoisted(() => ({
  listener: undefined as undefined | ((event: { payload: unknown }) => void),
  unlisten: vi.fn(), invoke: vi.fn(async () => null),
  createSession: vi.fn(async () => ({ id: "synthetic-new", model_id: "synthetic-model" })),
  dispatchSend: vi.fn(async () => ({ session_id: "synthetic-new", delivery: { status: "delivered", into: "new_turn" } })),
  getQuota: vi.fn(async () => [{ endpoint: "synthetic", five_hour_cap_percent: 80 }]),
  selectSession: vi.fn(async () => {}),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async (_name, listener) => { mocks.listener = listener; return mocks.unlisten; }) }));
vi.mock("./lib/tauri", () => ({ invoke: mocks.invoke }));
vi.mock("./lib/dispatchActions", () => ({ dispatchSend: mocks.dispatchSend, dispatchSetPermission: vi.fn(), dispatchSetModel: vi.fn(), dispatchStatus: vi.fn(), dispatchStop: vi.fn() }));
vi.mock("./lib/subscriptionQuota", () => ({ getQuota: mocks.getQuota, setQuotaCap: vi.fn() }));
vi.mock("./stores/chat", () => {
  const state = { activeSession: { id: "synthetic-existing" }, draftSession: null, activeModel: "synthetic-model", beginDraft: vi.fn(), loadSessions: vi.fn(), selectSession: mocks.selectSession, createSession: mocks.createSession };
  return { useChatStore: Object.assign((selector?: (s: typeof state) => unknown) => selector ? selector(state) : state, { getState: () => state }), openSessionId: () => "synthetic-existing" };
});
vi.mock("./stores/settings", () => ({ useSettingsStore: (selector: (s: unknown) => unknown) => selector({ load: vi.fn(), settings: { onboarded: true } }) }));
vi.mock("./stores/chatgptCatalog", () => ({ syncChatGptCatalog: vi.fn() }));
vi.mock("./pages/Workspace/WorkspacePage", () => ({ WorkspacePage: ({ onOpenSettings }: { onOpenSettings: () => void }) => <button onClick={onOpenSettings}>settings</button> }));
vi.mock("./pages/Settings/SettingsPage", () => ({ SettingsPage: () => <main>synthetic settings</main> }));
vi.mock("./pages/Resources/ResourcesPage", () => ({ ResourcesPage: () => null }));
vi.mock("./pages/ControlPlane/ControlPlanePage", () => ({ ControlPlanePage: () => null }));
vi.mock("./pages/Benchmarks/BenchmarksPage", () => ({ BenchmarksPage: () => null }));
vi.mock("./pages/Evolution/EvolutionWorkbenchPage", () => ({ EvolutionWorkbenchPage: () => null }));
vi.mock("./pages/Profile/ProfilePage", () => ({ ProfilePage: () => null }));
vi.mock("./components/Toast", () => ({ ToastContainer: () => null }));
vi.mock("./components/EvidenceViewer", () => ({ EvidenceViewer: () => null }));
vi.mock("./components/UpdaterBanner", () => ({ UpdaterBanner: () => null }));

 describe("M83 application dispatch lifetime", () => {
  it("answers quota and create_and_send while settings has unmounted the workspace, and switches back explicitly", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
    const app = render(<App />);
    await waitFor(() => expect(mocks.listener).toBeDefined());
    fireEvent.click(screen.getByText("settings"));
    expect(screen.getByText("synthetic settings")).toBeInTheDocument();
    expect(mocks.unlisten).not.toHaveBeenCalled();
    mocks.listener!({ payload: { request_id: "quota", operation: "get_quota", request: {} } });
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("dispatch_reply", expect.objectContaining({ requestId: "quota", ok: true, result: [{ endpoint: "synthetic", five_hour_cap_percent: 80 }] })));
    mocks.listener!({ payload: { request_id: "create", operation: "create_and_send", request: { project: "/synthetic/project", message: "synthetic message", delivery_authorized: false } } });
    await waitFor(() => expect(mocks.dispatchSend).toHaveBeenCalledWith({ sessionId: "synthetic-new", message: "synthetic message", mode: "steer", deliveryAuthorized: false }));
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("dispatch_reply", expect.objectContaining({ requestId: "create", ok: true })));
    expect(screen.getByText("synthetic settings")).toBeInTheDocument();
    mocks.listener!({ payload: { request_id: "switch", operation: "switch_session", request: { session_id: "synthetic-existing" } } });
    await waitFor(() => expect(screen.getByText("settings")).toBeInTheDocument());
    expect(mocks.selectSession).toHaveBeenCalledWith("synthetic-existing");
    app.unmount();
    expect(mocks.unlisten).toHaveBeenCalledTimes(1);
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });
});
