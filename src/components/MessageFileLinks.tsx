// SPDX-License-Identifier: Apache-2.0
// UI-layer helper shared by message rendering and tool-call cards: resolve a
// link's target to a local path and route every open through the native
// `open_chat_link` command, so schemes stay allow-listed and failures surface.
import { createContext } from "react";
import { invoke } from "../lib/tauri";

export const FileLinkScope = createContext<{ cwd?: string | null; sessionId?: string | null }>({});

export function resolveLocalPath(cwd: string | null | undefined, value: string): string {
  let path = value;
  if (/^file:\/\//i.test(path)) {
    path = path.slice(7);
    if (path.startsWith("localhost/")) path = path.slice(9);
    else if (!path.startsWith("/") && !/^[A-Za-z]:/.test(path)) path = `//${path}`;
    try { path = decodeURIComponent(path); } catch { /* keep malformed escapes literal */ }
    if (/^\/[A-Za-z]:[\\/]/.test(path)) path = path.slice(1);
  }
  if (path.startsWith("\\\\?\\UNC\\")) path = `\\\\${path.slice(8)}`;
  else if (path.startsWith("\\\\?\\")) path = path.slice(4);
  if (/^(\/|\\\\|[A-Za-z]:[\\/])/.test(path) || !cwd) return path;
  const sep = cwd.includes("\\") ? "\\" : "/";
  return `${cwd.replace(/[\\/]+$/, "")}${sep}${path.replace(/[\\/]/g, sep)}`;
}

/** Every UI opener uses this route; no shell-scheme capability expansion. */
export async function openFileLink(target: string, cwd?: string | null, sessionId?: string | null): Promise<void> {
  await invoke("open_chat_link", { target, cwd: cwd ?? null, sessionId: sessionId ?? null });
}

export function notifyFileLinkError(error: unknown): void {
  const previous = document.getElementById("file-link-error-toast");
  previous?.remove();
  const toast = document.createElement("div");
  toast.id = "file-link-error-toast";
  toast.setAttribute("role", "alert");
  toast.className = "fixed bottom-4 left-4 right-4 z-50 rounded-lg border border-status-danger/40 bg-surface-1 p-3 text-body text-status-danger shadow-lg";
  toast.textContent = `无法打开文件或链接：${String(error)}`;
  toast.onclick = () => toast.remove();
  document.body.append(toast);
  setTimeout(() => toast.remove(), 12000);
}

export function openFileLinkVisible(target: string, cwd?: string | null, sessionId?: string | null): void {
  void openFileLink(target, cwd, sessionId).catch(notifyFileLinkError);
}
