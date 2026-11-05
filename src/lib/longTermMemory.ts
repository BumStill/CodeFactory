// SPDX-License-Identifier: Apache-2.0
import { invoke } from "./tauri";

export interface LongTermMemory {
  id: string;
  content: string;
  kind: "preference" | "project_fact" | "decision";
  scope: "user" | "project";
  repo_label: string;
  source_session_id: string;
  source_kind: string;
  status: "active" | "superseded";
  created_at: number;
  source_text: string | null;
}

export interface LongTermMemoryOverview {
  enabled: boolean;
  memories: LongTermMemory[];
}

export function listLongTermMemories(): Promise<LongTermMemoryOverview> {
  return invoke<LongTermMemoryOverview>("list_long_term_memories");
}

export function editLongTermMemory(id: string, content: string): Promise<void> {
  return invoke<void>("edit_long_term_memory", { id, content });
}

export function deleteLongTermMemory(id: string): Promise<number> {
  return invoke<number>("delete_long_term_memory", { id });
}

export function setLongTermMemoryEnabled(enabled: boolean): Promise<void> {
  return invoke<void>("set_long_term_memory_enabled", { enabled });
}
