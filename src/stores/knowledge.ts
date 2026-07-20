// SPDX-License-Identifier: Apache-2.0
import { create } from "zustand";
import { invoke } from "../lib/tauri";
import type { KnowledgeLibrary, KnowledgeScanSummary } from "../lib/tauri";

interface KnowledgeState {
  libraries: KnowledgeLibrary[];
  scanSummaries: Record<string, KnowledgeScanSummary>;
  loading: boolean;
  scanning: Record<string, boolean>;
  error: string | null;

  loadLibraries: () => Promise<void>;
  registerLibrary: (name: string, rootPath: string) => Promise<KnowledgeLibrary>;
  scanLibrary: (libraryId: string) => Promise<KnowledgeScanSummary>;
  setLibraryEnabled: (libraryId: string, enabled: boolean) => Promise<void>;
  deleteLibrary: (libraryId: string) => Promise<void>;
}

export const useKnowledgeStore = create<KnowledgeState>((set, get) => ({
  libraries: [],
  scanSummaries: {},
  loading: false,
  scanning: {},
  error: null,

  loadLibraries: async () => {
    set({ loading: true, error: null });
    try {
      const libraries = await invoke<KnowledgeLibrary[]>("list_knowledge_libraries");
      set({ libraries, loading: false });
    } catch (e) {
      set({ loading: false, error: String(e) });
    }
  },

  registerLibrary: async (name, rootPath) => {
    set({ error: null });
    try {
      const library = await invoke<KnowledgeLibrary>("register_knowledge_library", {
        request: { name, root_path: rootPath },
      });
      await get().loadLibraries();
      return library;
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  scanLibrary: async (libraryId) => {
    set((s) => ({ scanning: { ...s.scanning, [libraryId]: true }, error: null }));
    try {
      const summary = await invoke<KnowledgeScanSummary>("scan_knowledge_library", {
        libraryId,
      });
      set((s) => ({
        scanSummaries: { ...s.scanSummaries, [libraryId]: summary },
        scanning: { ...s.scanning, [libraryId]: false },
      }));
      await get().loadLibraries();
      return summary;
    } catch (e) {
      set((s) => ({
        scanning: { ...s.scanning, [libraryId]: false },
        error: String(e),
      }));
      throw e;
    }
  },

  setLibraryEnabled: async (libraryId, enabled) => {
    set({ error: null });
    try {
      await invoke("set_knowledge_library_enabled", { libraryId, enabled });
      set((s) => ({
        libraries: s.libraries.map((library) =>
          library.id === libraryId ? { ...library, enabled } : library,
        ),
      }));
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  deleteLibrary: async (libraryId) => {
    set({ error: null });
    try {
      await invoke("delete_knowledge_library", { libraryId });
      set((s) => {
        const scanSummaries = { ...s.scanSummaries };
        delete scanSummaries[libraryId];
        return {
          libraries: s.libraries.filter((library) => library.id !== libraryId),
          scanSummaries,
        };
      });
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },
}));
