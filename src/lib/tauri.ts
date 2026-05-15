// SPDX-License-Identifier: Apache-2.0
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export { invoke };

export function onStream(
  sessionId: string,
  handler: (event: StreamEvent) => void
): Promise<UnlistenFn> {
  return listen<StreamEvent>(`stream:${sessionId}`, (e) => handler(e.payload));
}

// ── Types mirroring Rust StreamEvent ────────────────────────────────────────

export type StreamEvent =
  | { type: "text_delta"; content: string }
  | { type: "tool_call_start"; id: string; name: string; args: unknown }
  | { type: "tool_call_args_delta"; index: number; chunk: string }
  | { type: "tool_call_end"; index: number }
  | { type: "tool_result"; tool_call_id: string; content: string; is_error: boolean }
  | { type: "permission_request"; tool_call_id: string; tool_name: string; args: unknown }
  | { type: "done"; input_tokens: number; output_tokens: number }
  | { type: "error"; message: string };

export interface Session {
  id: string;
  title: string;
  cwd: string;
  model_id: string;
  created_at: number;
  updated_at: number;
  total_input_tokens: number;
  total_output_tokens: number;
}

export interface Message {
  id: string;
  session_id: string;
  role: "user" | "assistant" | "tool" | "system";
  content: string;
  model_id?: string;
  input_tokens?: number;
  output_tokens?: number;
  created_at: number;
}

export interface ModelInfo {
  id: string;
  name: string;
  context_length: number;
  pricing?: { prompt: string; completion: string };
}

export interface Settings {
  endpoints: Record<string, { base_url: string; key_ref?: string }>;
  default_endpoint: string;
  default_model: string;
  permissions: {
    allow: string[];
    ask: string[];
    deny: string[];
    full_access: boolean;
  };
  shell: {
    shell: string;
  };
}
