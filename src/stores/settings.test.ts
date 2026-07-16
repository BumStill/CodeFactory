// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("../lib/tauri", async () => {
  const real = await vi.importActual<typeof import("../lib/tauri")>("../lib/tauri");
  return { ...real, invoke: mocks.invoke };
});

import type { Settings } from "../lib/tauri";
import { useSettingsStore } from "./settings";

describe("settings store authority", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
    useSettingsStore.setState({ settings: null });
  });

  it("adopts the backend-merged settings returned by save_settings", async () => {
    const requested = {
      theme: "dark",
      font_family: "inter",
      font_size: 14,
      default_endpoint: "chatgpt",
      default_model: "stale-model",
      endpoints: {},
    } as Settings;
    const authoritative = {
      ...requested,
      default_model: "gpt-5.6-sol",
      endpoints: {
        chatgpt: {
          base_url: "https://chatgpt.com/backend-api/codex",
          api_style: "chatgpt",
          active_model: "gpt-5.6-sol",
          custom_models: [],
        },
      },
    } as Settings;
    mocks.invoke.mockResolvedValue(authoritative);

    await useSettingsStore.getState().save(requested);

    expect(mocks.invoke).toHaveBeenCalledWith("save_settings", {
      newSettings: requested,
    });
    expect(useSettingsStore.getState().settings).toEqual(authoritative);
  });
});
