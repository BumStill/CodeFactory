// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "react";
import { invoke } from "../lib/tauri";
import { useChatStore } from "../stores/chat";
type Urgency = "default" | "immediate" | "hold";
export function ReleaseUrgencyPicker() {
  const sessionId = useChatStore((s) => s.activeSession?.kind === "project" ? s.activeSession.id : undefined);
  const [urgency, setUrgency] = useState<Urgency>("default");
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => {
    let live = true;
    setLoaded(false); setError("");
    if (sessionId) void invoke<Urgency>("get_session_release_urgency", { sessionId }).then((value) => {
      if (live) { setUrgency(value); setLoaded(true); }
    }).catch((e) => { if (live) setError(String(e)); });
    return () => { live = false; };
  }, [sessionId]);
  if (!sessionId) return null;
  return <div className="space-y-1 px-2 py-2">
    <label className="block text-caption text-gray-400">发版紧急度
      <select aria-label="发版紧急度" disabled={!loaded} value={urgency}
        className="min-h-[44px] w-full rounded border border-control-border bg-surface-3 px-2 py-1 text-label text-gray-200 outline-none focus:border-accent"
        onChange={async (event) => {
          const value = event.target.value as Urgency;
          setLoaded(false); setError("");
          try { await invoke("update_session_release_urgency", { sessionId, urgency: value }); setUrgency(value); }
          catch (e) { setError(String(e)); }
          finally { setLoaded(true); }
        }}>
        <option value="default">默认（随批次发版）</option>
        <option value="immediate">立即发版</option>
        <option value="hold">暂缓发版</option>
      </select>
    </label>
    {error && <p role="alert" className="text-caption text-status-error">{error}</p>}
  </div>;
}
