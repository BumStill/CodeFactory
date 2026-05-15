// SPDX-License-Identifier: Apache-2.0
import { create } from "zustand";
import { invoke } from "../lib/tauri";

export interface SpecMeta {
  req_id: string | null;
  title: string;
  status: "draft" | "review" | "approved" | "implementing" | "done" | string;
  created_at: string;
  updated_at: string;
  tags: string[];
  acceptance_criteria: string[];
  file_path: string;
  rel_path: string;
}

export interface SpecFile {
  meta: SpecMeta;
  content: string; // full markdown including frontmatter
  body: string;    // body without frontmatter
}

interface SpecsStore {
  specs: SpecMeta[];
  activeSpec: SpecFile | null;
  loading: boolean;
  error: string | null;

  loadSpecs: (cwd: string) => Promise<void>;
  openSpec: (path: string) => Promise<void>;
  saveSpec: (path: string, content: string) => Promise<SpecMeta>;
  createSpec: (cwd: string, title: string) => Promise<SpecFile>;
  deleteSpec: (path: string) => Promise<void>;
  approveSpec: (path: string) => Promise<SpecMeta>;
  setActiveSpec: (spec: SpecFile | null) => void;
  updateActiveContent: (content: string) => void;
}

export const useSpecsStore = create<SpecsStore>((set, _get) => ({
  specs: [],
  activeSpec: null,
  loading: false,
  error: null,

  loadSpecs: async (cwd) => {
    set({ loading: true, error: null });
    try {
      const specs = await invoke<SpecMeta[]>("list_specs", { cwd });
      set({ specs, loading: false });
    } catch (e) {
      set({ loading: false, error: String(e) });
    }
  },

  openSpec: async (path) => {
    set({ loading: true, error: null });
    try {
      const spec = await invoke<SpecFile>("get_spec", { path });
      set({ activeSpec: spec, loading: false });
    } catch (e) {
      set({ loading: false, error: String(e) });
    }
  },

  saveSpec: async (path, content) => {
    const meta = await invoke<SpecMeta>("save_spec", { path, content });
    // Update the active spec meta if it's the same file
    set((s) => ({
      activeSpec:
        s.activeSpec?.meta.file_path === path
          ? { ...s.activeSpec, meta, content }
          : s.activeSpec,
      specs: s.specs.map((m) => (m.file_path === path ? meta : m)),
    }));
    return meta;
  },

  createSpec: async (cwd, title) => {
    const spec = await invoke<SpecFile>("create_spec", { cwd, title });
    set((s) => ({ specs: [spec.meta, ...s.specs], activeSpec: spec }));
    return spec;
  },

  deleteSpec: async (path) => {
    await invoke("delete_spec", { path });
    set((s) => ({
      specs: s.specs.filter((m) => m.file_path !== path),
      activeSpec: s.activeSpec?.meta.file_path === path ? null : s.activeSpec,
    }));
  },

  approveSpec: async (path) => {
    const meta = await invoke<SpecMeta>("approve_spec", { path });
    set((s) => ({
      activeSpec:
        s.activeSpec?.meta.file_path === path
          ? { ...s.activeSpec, meta }
          : s.activeSpec,
      specs: s.specs.map((m) => (m.file_path === path ? meta : m)),
    }));
    return meta;
  },

  setActiveSpec: (spec) => set({ activeSpec: spec }),

  updateActiveContent: (content) =>
    set((s) =>
      s.activeSpec
        ? { activeSpec: { ...s.activeSpec, content } }
        : {}
    ),
}));
