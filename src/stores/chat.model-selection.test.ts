// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useChatStore } from "./chat";
import { useSettingsStore } from "./settings";
const mocks = vi.hoisted(() => ({ invoke: vi.fn(), onStream: vi.fn(), onSessionUpdated: vi.fn() }));
vi.mock("../lib/tauri", () => ({ ...mocks, sendMessageAnonymous: vi.fn() }));
function defaults(model: string) {
  useSettingsStore.setState({ settings: { default_endpoint: "synthetic", default_model: model, endpoints: { synthetic: { active_model: model } } } as never });
}
describe("CF-MSH-R1/R2 model decision", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    defaults("sol");
    useChatStore.setState({ activeModel: "luna", activeSession: null, draftSession: null, sessions: [], runtime: {}, _draftMaterialization: null, _unlisten: {}, _unlistenSessionUpdated: {} });
    mocks.onStream.mockResolvedValue(() => {});
    mocks.onSessionUpdated.mockResolvedValue(() => {});
    mocks.invoke.mockImplementation(async (cmd, args) => cmd === "materialize_draft_session" ? { id: args.draftId, model_id: args.modelId, title: "synthetic", permission_mode: "standard", cwd: "/tmp/synthetic", kind: "quick", created_at: 1, updated_at: 1 } : undefined);
  });
  it.each(["sol", "opus", "flash"])("defaults and persisted restart settings seed %s, not cached old selection", (model) => {
    defaults(model);
    const draft = useChatStore.getState().beginDraft();
    expect(draft.modelId).toBe(model);
    expect(useChatStore.getState().activeModel).toBe(model);
  });
  it("changed default seeds the next draft after an existing model was selected", () => {
    useChatStore.getState().beginDraft();
    useChatStore.getState().setModel("explicit");
    defaults("new-default");
    expect(useChatStore.getState().beginDraft().modelId).toBe("new-default");
  });
  it("async default seed and explicit selection update displayed and admitted models together", async () => {
    useChatStore.getState().beginDraft();
    useChatStore.getState().setModel("sol");
    expect(useChatStore.getState().draftSession?.modelId).toBe("sol");
    await useChatStore.getState().updateActiveSessionModel("explicit");
    await useChatStore.getState().sendOrQueue("整理合成数据模型");
    expect(mocks.invoke).toHaveBeenCalledWith("materialize_draft_session", expect.objectContaining({ modelId: "explicit" }));
    expect(useChatStore.getState().activeSession?.model_id).toBe("explicit");
  });
});
