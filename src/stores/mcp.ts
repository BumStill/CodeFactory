// SPDX-License-Identifier: Apache-2.0
import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";

export interface McpServerConfig {
  id: string;
  name: string;
  command: string;
  args: string[];
  env: Record<string, string>;
  enabled: boolean;
}

export interface McpTool {
  name: string;
  description: string;
  input_schema: Record<string, unknown>;
  server_id: string;
}

interface McpStore {
  servers: McpServerConfig[];
  tools: McpTool[];
  loading: boolean;
  error: string | null;

  loadServers: () => Promise<void>;
  loadTools: () => Promise<void>;
  addServer: (config: McpServerConfig) => Promise<void>;
  updateServer: (id: string, config: McpServerConfig) => Promise<void>;
  deleteServer: (id: string) => Promise<void>;
  enableServer: (id: string) => Promise<McpTool[]>;
  disableServer: (id: string) => Promise<void>;
  testTool: (serverId: string, toolName: string, args: Record<string, unknown>) => Promise<string>;
}

export const useMcpStore = create<McpStore>((set, get) => ({
  servers: [],
  tools: [],
  loading: false,
  error: null,

  loadServers: async () => {
    set({ loading: true, error: null });
    try {
      const servers = await invoke<McpServerConfig[]>("list_mcp_servers");
      set({ servers, loading: false });
    } catch (e) {
      set({ error: String(e), loading: false });
    }
  },

  loadTools: async () => {
    try {
      const tools = await invoke<McpTool[]>("list_mcp_tools");
      set({ tools });
    } catch (e) {
      set({ error: String(e) });
    }
  },

  addServer: async (config) => {
    await invoke("add_mcp_server", { config });
    await get().loadServers();
    await get().loadTools();
  },

  updateServer: async (id, config) => {
    await invoke("update_mcp_server", { id, config });
    await get().loadServers();
    await get().loadTools();
  },

  deleteServer: async (id) => {
    await invoke("delete_mcp_server", { id });
    await get().loadServers();
    await get().loadTools();
  },

  enableServer: async (id) => {
    const tools = await invoke<McpTool[]>("enable_mcp_server", { id });
    await get().loadServers();
    await get().loadTools();
    return tools;
  },

  disableServer: async (id) => {
    await invoke("disable_mcp_server", { id });
    await get().loadServers();
    await get().loadTools();
  },

  testTool: async (serverId, toolName, args) => {
    return invoke<string>("test_mcp_tool", {
      serverId,
      toolName,
      args,
    });
  },
}));
