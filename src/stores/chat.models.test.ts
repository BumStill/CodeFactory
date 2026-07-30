// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useChatStore } from "./chat";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
}));

vi.mock("../lib/tauri", () => ({
  invoke: mocks.invoke,
  onStream: vi.fn(),
  onSessionUpdated: vi.fn(),
  sendMessageAnonymous: vi.fn(),
}));

describe("chat store model loading", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
    useChatStore.setState({ models: [] });
  });

  it("does not let an older endpoint response replace the latest endpoint models", async () => {
    let resolveChatgpt:
      | ((models: Array<{ id: string; name: string }>) => void)
      | undefined;
    let resolveDeepseek:
      | ((models: Array<{ id: string; name: string }>) => void)
      | undefined;

    mocks.invoke.mockImplementation((_command: string, args: { endpointName: string }) => {
      return new Promise((resolve) => {
        if (args.endpointName === "chatgpt") {
          resolveChatgpt = resolve;
        } else if (args.endpointName === "deepseek") {
          resolveDeepseek = resolve;
        }
      });
    });

    const chatgptLoad = useChatStore.getState().loadModels("chatgpt");
    const deepseekLoad = useChatStore.getState().loadModels("deepseek");

    resolveDeepseek?.([{ id: "deepseek-v4-pro", name: "DeepSeek V4 Pro" }]);
    await deepseekLoad;
    expect(useChatStore.getState().models.map((model) => model.id)).toEqual([
      "deepseek-v4-pro",
    ]);

    resolveChatgpt?.([{ id: "gpt-5.6-sol", name: "GPT-5.6 Sol" }]);
    await chatgptLoad;

    expect(useChatStore.getState().models.map((model) => model.id)).toEqual([
      "deepseek-v4-pro",
    ]);
  });
});
