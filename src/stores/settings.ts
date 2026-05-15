// SPDX-License-Identifier: Apache-2.0
import { create } from "zustand";
import { invoke } from "../lib/tauri";
import type { Settings } from "../lib/tauri";

interface SettingsStore {
  settings: Settings | null;
  load: () => Promise<void>;
  save: (s: Settings) => Promise<void>;
  saveApiKey: (keyRef: string, value: string) => Promise<void>;
  getApiKey: (keyRef: string) => Promise<string | null>;
}

export const useSettingsStore = create<SettingsStore>((set) => ({
  settings: null,

  load: async () => {
    const s = await invoke<Settings>("get_settings");
    set({ settings: s });
  },

  save: async (s) => {
    await invoke("save_settings", { newSettings: s });
    set({ settings: s });
  },

  saveApiKey: async (keyRef, value) => {
    await invoke("save_api_key", { keyRef, value });
  },

  getApiKey: async (keyRef) => {
    return invoke<string | null>("get_api_key", { keyRef });
  },
}));
