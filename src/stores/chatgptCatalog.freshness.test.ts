// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ codexAccount: vi.fn(), codexModels: vi.fn(), applyCodexModels: vi.fn() }));
vi.mock("../lib/tauri", () => ({
  codexAccount: mocks.codexAccount,
  codexModels: mocks.codexModels,
  applyCodexModels: mocks.applyCodexModels,
}));
const settingsState = vi.hoisted(() => ({ settings: null as unknown, load: vi.fn() }));
vi.mock("./settings", () => ({
  useSettingsStore: { getState: () => settingsState },
}));

import { refreshChatGptCatalogIfStale, resetChatGptCatalogFreshness } from "./chatgptCatalog";

describe("catalog freshness window", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    resetChatGptCatalogFreshness();
    mocks.codexAccount.mockResolvedValue({ email: "a@b" });
    mocks.codexModels.mockResolvedValue([{ id: "gpt-6-astra" }]);
    settingsState.settings = null; // sync returns early after fetching
  });

  it("asks the server the first time the picker opens", async () => {
    await refreshChatGptCatalogIfStale();
    expect(mocks.codexModels).toHaveBeenCalledTimes(1);
  });

  it("does not re-ask inside the freshness window", async () => {
    await refreshChatGptCatalogIfStale();
    await refreshChatGptCatalogIfStale();
    await refreshChatGptCatalogIfStale();
    expect(mocks.codexModels).toHaveBeenCalledTimes(1);
  });

  it("asks again once the window has passed", async () => {
    await refreshChatGptCatalogIfStale();
    vi.setSystemTime(new Date(Date.now() + 6 * 60 * 1000));
    await refreshChatGptCatalogIfStale();
    expect(mocks.codexModels).toHaveBeenCalledTimes(2);
    vi.useRealTimers();
  });

  it("collapses concurrent opens into one request", async () => {
    let release: (v: unknown) => void = () => {};
    mocks.codexModels.mockReturnValue(new Promise((resolve) => { release = resolve; }));
    const a = refreshChatGptCatalogIfStale();
    const b = refreshChatGptCatalogIfStale();
    release([{ id: "gpt-6-astra" }]);
    await Promise.all([a, b]);
    expect(mocks.codexModels).toHaveBeenCalledTimes(1);
  });

  it("stays quiet when signed out", async () => {
    resetChatGptCatalogFreshness();
    mocks.codexAccount.mockResolvedValue(null);
    await refreshChatGptCatalogIfStale();
    expect(mocks.codexModels).not.toHaveBeenCalled();
  });
});
