// SPDX-License-Identifier: Apache-2.0
import { create } from "zustand";
import { invoke } from "../lib/tauri";

export interface SkillManifest {
  id: string;
  name: string;
  description: string;
  version: string;
  author: string;
  tags: string[];
  enabled: boolean;
  path: string;
  source: "builtin" | "user";
}

export interface SlashCommand {
  name: string;
  description: string;
  template: string;
}

export interface SkillDetail {
  manifest: SkillManifest;
  system_prompt: string;
  slash_commands: SlashCommand[];
  has_tool_policy: boolean;
}

interface SkillsStore {
  skills: SkillManifest[];
  loading: boolean;
  loadSkills: () => Promise<void>;
  enableSkill: (id: string) => Promise<void>;
  disableSkill: (id: string) => Promise<void>;
  installFromUrl: (url: string) => Promise<void>;
  importFromDirectory: (dirPath: string) => Promise<number>;
  createSkill: (name: string, description: string, instructions: string) => Promise<void>;
  updateSkill: (
    id: string,
    fields: { name?: string; description?: string; instructions?: string },
  ) => Promise<void>;
  deleteSkill: (id: string) => Promise<void>;
  getSkillDetail: (id: string) => Promise<SkillDetail>;
}

export const useSkillsStore = create<SkillsStore>((set) => ({
  skills: [],
  loading: false,

  loadSkills: async () => {
    set({ loading: true });
    try {
      const skills = await invoke<SkillManifest[]>("list_skills");
      set({ skills });
    } finally {
      set({ loading: false });
    }
  },

  enableSkill: async (id) => {
    await invoke("enable_skill", { id });
    const skills = await invoke<SkillManifest[]>("list_skills");
    set({ skills });
  },

  disableSkill: async (id) => {
    await invoke("disable_skill", { id });
    const skills = await invoke<SkillManifest[]>("list_skills");
    set({ skills });
  },

  installFromUrl: async (url) => {
    await invoke("install_skill_from_url", { url });
    const skills = await invoke<SkillManifest[]>("list_skills");
    set({ skills });
  },

  importFromDirectory: async (dirPath) => {
    const imported = await invoke<SkillManifest[]>("install_skill_from_directory", { dirPath });
    const skills = await invoke<SkillManifest[]>("list_skills");
    set({ skills });
    return imported.length;
  },

  createSkill: async (name, description, instructions) => {
    await invoke("create_skill", { name, description, instructions });
    const skills = await invoke<SkillManifest[]>("list_skills");
    set({ skills });
  },

  updateSkill: async (id, fields) => {
    await invoke("update_skill", { id, ...fields });
    const skills = await invoke<SkillManifest[]>("list_skills");
    set({ skills });
  },

  deleteSkill: async (id) => {
    await invoke("delete_skill", { id });
    const skills = await invoke<SkillManifest[]>("list_skills");
    set({ skills });
  },

  getSkillDetail: async (id) => {
    return invoke<SkillDetail>("get_skill", { id });
  },
}));
