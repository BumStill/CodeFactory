// SPDX-License-Identifier: Apache-2.0
import { create } from "zustand";
import { invoke } from "../lib/tauri";
import type { GitBranch, GitCommit, GitStatus } from "../lib/tauri";

interface GitState {
  cwd: string | null;
  status: GitStatus | null;
  branches: GitBranch[];
  commits: GitCommit[];
  refreshing: boolean;
  lastRefresh: number | null;
  error: string | null;

  setCwd: (cwd: string | null) => void;
  refreshStatus: () => Promise<void>;
  refreshBranches: () => Promise<void>;
  refreshCommits: (limit?: number) => Promise<void>;
  refreshAll: () => Promise<void>;
  stageFiles: (files: string[]) => Promise<void>;
  commit: (message: string) => Promise<void>;
  checkout: (target: string) => Promise<void>;
  createBranch: (name: string, checkout: boolean) => Promise<void>;
  getDiff: (file?: string) => Promise<string>;
  getFileDiff: (file: string, staged: boolean) => Promise<string>;
}

export const useGitStore = create<GitState>((set, get) => ({
  cwd: null,
  status: null,
  branches: [],
  commits: [],
  refreshing: false,
  lastRefresh: null,
  error: null,

  setCwd: (cwd) => {
    const prev = get().cwd;
    if (prev === cwd) return;
    set({ cwd, status: null, branches: [], commits: [], lastRefresh: null, error: null });
  },

  refreshStatus: async () => {
    const cwd = get().cwd;
    if (!cwd) return;
    set({ refreshing: true });
    try {
      const status = await invoke<GitStatus>("git_status", { cwd });
      set({ status, refreshing: false, lastRefresh: Date.now(), error: null });
    } catch (e) {
      set({ refreshing: false, error: String(e) });
    }
  },

  refreshBranches: async () => {
    const cwd = get().cwd;
    if (!cwd) return;
    try {
      const branches = await invoke<GitBranch[]>("git_branches", { cwd });
      set({ branches });
    } catch (e) {
      set({ error: String(e) });
    }
  },

  refreshCommits: async (limit = 50) => {
    const cwd = get().cwd;
    if (!cwd) return;
    try {
      const commits = await invoke<GitCommit[]>("git_log", { cwd, limit });
      set({ commits });
    } catch (e) {
      set({ error: String(e) });
    }
  },

  refreshAll: async () => {
    await Promise.all([
      get().refreshStatus(),
      get().refreshBranches(),
      get().refreshCommits(),
    ]);
  },

  stageFiles: async (files) => {
    const cwd = get().cwd;
    if (!cwd || files.length === 0) return;
    await invoke("git_add", { cwd, files });
    await get().refreshStatus();
  },

  commit: async (message) => {
    const cwd = get().cwd;
    if (!cwd) throw new Error("No active workspace");
    await invoke<string>("git_commit", { cwd, message });
    await Promise.all([get().refreshStatus(), get().refreshCommits()]);
  },

  checkout: async (target) => {
    const cwd = get().cwd;
    if (!cwd) throw new Error("No active workspace");
    await invoke("git_checkout", { cwd, target });
    await get().refreshAll();
  },

  createBranch: async (name, checkout) => {
    const cwd = get().cwd;
    if (!cwd) throw new Error("No active workspace");
    await invoke("git_create_branch", { cwd, name, checkout });
    await get().refreshAll();
  },

  getDiff: async (file) => {
    const cwd = get().cwd;
    if (!cwd) return "";
    return invoke<string>("git_diff", { cwd, file: file ?? null });
  },

  getFileDiff: async (file, staged) => {
    const cwd = get().cwd;
    if (!cwd) return "";
    return invoke<string>("git_file_diff", { cwd, file, staged });
  },
}));
