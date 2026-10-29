// SPDX-License-Identifier: Apache-2.0
import { create } from "zustand";
import { invoke } from "../lib/tauri";
import type {
  KnowledgeDocumentFailure,
  KnowledgeLibrary,
  KnowledgeScanSummary,
} from "../lib/tauri";

interface KnowledgeState {
  libraries: KnowledgeLibrary[];
  scanSummaries: Record<string, KnowledgeScanSummary>;
  loading: boolean;
  scanning: Record<string, boolean>;
  /** Files that currently fail to index, per library (M191 R6). */
  failures: Record<string, KnowledgeDocumentFailure[]>;
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
  failures: {},
  error: null,

  loadLibraries: async () => {
    set({ loading: true, error: null });
    try {
      const libraries = await invoke<KnowledgeLibrary[]>("list_knowledge_libraries");
      // Auto-indexing happens in the backend, so refresh the per-file failure
      // list alongside the libraries; the panel polls this while mounted.
      const failures: Record<string, KnowledgeDocumentFailure[]> = {};
      await Promise.all(
        libraries.map(async (library) => {
          try {
            failures[library.id] = await invoke<KnowledgeDocumentFailure[]>(
              "list_knowledge_library_failures",
              { libraryId: library.id },
            );
          } catch {
            failures[library.id] = [];
          }
        }),
      );
      set({ libraries, failures, loading: false });
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
        const failures = { ...s.failures };
        delete failures[libraryId];
        return {
          libraries: s.libraries.filter((library) => library.id !== libraryId),
          scanSummaries,
          failures,
        };
      });
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },
}));
