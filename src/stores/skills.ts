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
  lifecycle_status?: "ready" | "corrupt";
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
  tool_policy: string | null;
  review_fingerprint: string | null;
}

export interface SkillImportFailure {
  path: string;
  error: string;
}

export interface SkillImportResult {
  succeeded: SkillManifest[];
  failed: SkillImportFailure[];
}

export interface SkillSourceSelection {
  source_handle: string;
  display_path: string;
}

interface SkillsStore {
  skills: SkillManifest[];
  loading: boolean;
  catalogError: string | null;
  loadSkills: () => Promise<void>;
  enableSkill: (id: string, expectedReviewFingerprint: string) => Promise<void>;
  disableSkill: (id: string) => Promise<void>;
  installFromUrl: (url: string) => Promise<SkillManifest>;
  installMarketplace: (skillId: string) => Promise<SkillManifest>;
  selectSourceDirectory: () => Promise<SkillSourceSelection | null>;
  importFromDirectory: (sourceHandle: string) => Promise<SkillImportResult>;
  createSkill: (name: string, description: string, instructions: string) => Promise<SkillManifest>;
  updateSkill: (
    id: string,
    fields: { name?: string; description?: string; instructions?: string },
  ) => Promise<void>;
  deleteSkill: (id: string) => Promise<void>;
  getSkillDetail: (id: string) => Promise<SkillDetail>;
}

function upsertSkills(current: SkillManifest[], additions: SkillManifest[]): SkillManifest[] {
  const byId = new Map(current.map((skill) => [skill.id, skill]));
  for (const skill of additions) byId.set(skill.id, skill);
  return [...byId.values()].sort((a, b) => a.name.localeCompare(b.name));
}

export const useSkillsStore = create<SkillsStore>((set) => {
  // Every catalog request or successful mutation advances this epoch. A stale
  // list response must never overwrite a newer mutation receipt or snapshot.
  let catalogEpoch = 0;

  const beginMutationReceipt = (): number => {
    catalogEpoch += 1;
    return catalogEpoch;
  };

  const refreshSkillsAfterMutation = async (receiptEpoch: number): Promise<void> => {
    try {
      const skills = await invoke<SkillManifest[]>("list_skills");
      if (receiptEpoch === catalogEpoch) set({ skills, catalogError: null });
    } catch (error) {
      // The mutation receipt is authoritative for the just-changed item. Keep
      // that local result visible; a catalog refresh can be retried separately.
      if (receiptEpoch === catalogEpoch) {
        set({ catalogError: `技能目录刷新失败：${String(error)}` });
      }
    }
  };

  return {
  skills: [],
  loading: false,
  catalogError: null,

  loadSkills: async () => {
    const requestEpoch = ++catalogEpoch;
    set({ loading: true, catalogError: null });
    try {
      const skills = await invoke<SkillManifest[]>("list_skills");
      if (requestEpoch === catalogEpoch) set({ skills, catalogError: null });
    } catch (error) {
      if (requestEpoch === catalogEpoch) {
        set({ catalogError: `技能目录加载失败：${String(error)}` });
      }
      throw error;
    } finally {
      if (requestEpoch === catalogEpoch) set({ loading: false });
    }
  },

  enableSkill: async (id, expectedReviewFingerprint) => {
    await invoke("enable_skill", { id, expectedReviewFingerprint });
    const receiptEpoch = beginMutationReceipt();
    set((state) => ({
      skills: state.skills.map((skill) => skill.id === id ? { ...skill, enabled: true } : skill),
      loading: false,
    }));
    await refreshSkillsAfterMutation(receiptEpoch);
  },

  disableSkill: async (id) => {
    await invoke("disable_skill", { id });
    const receiptEpoch = beginMutationReceipt();
    set((state) => ({
      skills: state.skills.map((skill) => skill.id === id ? { ...skill, enabled: false } : skill),
      loading: false,
    }));
    await refreshSkillsAfterMutation(receiptEpoch);
  },

  installFromUrl: async (url) => {
    const installed = await invoke<SkillManifest>("install_skill_from_url", { url });
    const receiptEpoch = beginMutationReceipt();
    set((state) => ({ skills: upsertSkills(state.skills, [installed]), loading: false }));
    await refreshSkillsAfterMutation(receiptEpoch);
    return installed;
  },

  installMarketplace: async (skillId) => {
    const installed = await invoke<SkillManifest>("install_marketplace_skill", { skillId });
    const receiptEpoch = beginMutationReceipt();
    set((state) => ({ skills: upsertSkills(state.skills, [installed]), loading: false }));
    await refreshSkillsAfterMutation(receiptEpoch);
    return installed;
  },

  selectSourceDirectory: async () => {
    return invoke<SkillSourceSelection | null>("select_skill_source_directory");
  },

  importFromDirectory: async (sourceHandle) => {
    const imported = await invoke<SkillImportResult>("install_skill_from_directory", { sourceHandle });
    const receiptEpoch = beginMutationReceipt();
    set((state) => ({ skills: upsertSkills(state.skills, imported.succeeded), loading: false }));
    await refreshSkillsAfterMutation(receiptEpoch);
    return imported;
  },

  createSkill: async (name, description, instructions) => {
    const created = await invoke<SkillManifest>("create_skill", { name, description, instructions });
    const receiptEpoch = beginMutationReceipt();
    set((state) => ({ skills: upsertSkills(state.skills, [created]), loading: false }));
    await refreshSkillsAfterMutation(receiptEpoch);
    return created;
  },

  updateSkill: async (id, fields) => {
    const updated = await invoke<SkillManifest>("update_skill", { id, ...fields });
    const receiptEpoch = beginMutationReceipt();
    set((state) => ({ skills: upsertSkills(state.skills, [updated]), loading: false }));
    await refreshSkillsAfterMutation(receiptEpoch);
  },

  deleteSkill: async (id) => {
    await invoke("delete_skill", { id });
    const receiptEpoch = beginMutationReceipt();
    set((state) => ({ skills: state.skills.filter((skill) => skill.id !== id), loading: false }));
    await refreshSkillsAfterMutation(receiptEpoch);
  },

  getSkillDetail: async (id) => {
    return invoke<SkillDetail>("get_skill", { id });
  },
  };
});
