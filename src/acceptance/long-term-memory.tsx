// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for the M235 memory page.
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import "../styles/globals.css";
import { LongTermMemorySection } from "../components/LongTermMemorySection";

const theme = new URLSearchParams(location.search).get("theme") === "light" ? "light" : "dark";
document.documentElement.dataset.theme = theme;
const base = { source_session_id: "synthetic", source_kind: "user_message", created_at: 1_790_000_000_000 };
let enabled = true;
let memories = [
  { ...base, id: "a", content: "回复用中文", kind: "preference", scope: "user", repo_label: "", status: "active", source_text: "记住：回复用中文" },
  { ...base, id: "b", content: "这个项目的回复结尾不要总结", kind: "project_fact", scope: "project", repo_label: "codefactory", status: "active", source_text: "记住：这个项目的回复结尾不要总结" },
  { ...base, id: "c", content: "用 npm", kind: "decision", scope: "project", repo_label: "codefactory", status: "superseded", source_text: null },
];
mockIPC((command, args) => {
  const input = args as { id?: string; enabled?: boolean; content?: string };
  if (command === "list_long_term_memories") return { enabled, memories };
  if (command === "delete_long_term_memory") { memories = memories.filter((memory) => memory.id !== input.id); return 1; }
  if (command === "set_long_term_memory_enabled") { enabled = input.enabled ?? enabled; return null; }
  if (command === "edit_long_term_memory") {
    if (input.content?.includes("token=")) throw "内容里有密钥或凭据，不能存入长期记忆";
    memories = memories.map((memory) => memory.id === input.id ? { ...memory, content: input.content ?? memory.content } : memory);
    return null;
  }
  return null;
});
createRoot(document.getElementById("root")!).render(<main className="min-h-screen bg-surface-0 p-4"><LongTermMemorySection /></main>);
