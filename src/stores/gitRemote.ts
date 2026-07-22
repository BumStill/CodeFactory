// SPDX-License-Identifier: Apache-2.0
import { create } from "zustand";
import { invoke } from "../lib/tauri";
import type {
  AddGitRemoteRequest,
  GitRemoteConfig,
  RemoteIssue,
  RemotePR,
  RemoteRepo,
} from "../lib/tauri";

interface GitRemoteStore {
  remotes: GitRemoteConfig[];
  issues: RemoteIssue[];
  prs: RemotePR[];
  repos: RemoteRepo[];
  loading: boolean;
  error: string | null;

  loadRemotes: () => Promise<void>;
  addRemote: (config: AddGitRemoteRequest) => Promise<void>;
  deleteRemote: (id: string) => Promise<void>;
  testRemote: (id: string) => Promise<string>;
  loadIssues: (remoteId: string, repo: string, state: string) => Promise<void>;
  loadPRs: (remoteId: string, repo: string, state: string) => Promise<void>;
  loadRepos: (remoteId: string) => Promise<void>;
  createIssue: (remoteId: string, repo: string, title: string, body: string, labels: string[]) => Promise<RemoteIssue>;
  createPR: (remoteId: string, repo: string, title: string, body: string, head: string, base: string, draft: boolean) => Promise<RemotePR>;
}

export const useGitRemoteStore = create<GitRemoteStore>((set) => ({
  remotes: [],
  issues: [],
  prs: [],
  repos: [],
  loading: false,
  error: null,

  loadRemotes: async () => {
    try {
      const remotes = await invoke<GitRemoteConfig[]>("list_git_remotes");
      set({ remotes });
    } catch (e) {
      set({ error: String(e) });
    }
  },

  addRemote: async (config) => {
    await invoke("add_git_remote", { config });
    const remotes = await invoke<GitRemoteConfig[]>("list_git_remotes");
    set({ remotes });
  },

  deleteRemote: async (id) => {
    await invoke("delete_git_remote", { id });
    set((s) => ({ remotes: s.remotes.filter((r) => r.id !== id) }));
  },

  testRemote: async (id) => {
    return await invoke<string>("test_git_remote", { id });
  },

  loadIssues: async (remoteId, repo, state) => {
    set({ loading: true, error: null });
    try {
      const issues = await invoke<RemoteIssue[]>("list_issues", {
        remoteId,
        repo,
        stateFilter: state,
      });
      set({ issues, loading: false });
    } catch (e) {
      set({ loading: false, error: String(e) });
    }
  },

  loadPRs: async (remoteId, repo, state) => {
    set({ loading: true, error: null });
    try {
      const prs = await invoke<RemotePR[]>("list_prs", {
        remoteId,
        repo,
        stateFilter: state,
      });
      set({ prs, loading: false });
    } catch (e) {
      set({ loading: false, error: String(e) });
    }
  },

  loadRepos: async (remoteId) => {
    set({ loading: true, error: null });
    try {
      const repos = await invoke<RemoteRepo[]>("list_repos", { remoteId });
      set({ repos, loading: false });
    } catch (e) {
      set({ loading: false, error: String(e) });
    }
  },

  createIssue: async (remoteId, repo, title, body, labels) => {
    return await invoke<RemoteIssue>("create_issue", {
      remoteId,
      repo,
      title,
      body,
      labels,
    });
  },

  createPR: async (remoteId, repo, title, body, head, base, draft) => {
    return await invoke<RemotePR>("create_pr", {
      remoteId,
      repo,
      title,
      body,
      head,
      base,
      draft,
    });
  },
}));
