// SPDX-License-Identifier: Apache-2.0
import { useState, useRef, useEffect } from "react";
import { ChevronDown, Sparkles } from "lucide-react";
import { useChatStore } from "../stores/chat";
import { useSettingsStore } from "../stores/settings";

export function ModelPicker() {
  const { models, activeModel, updateActiveSessionModel, loadModels } = useChatStore();
  const { settings } = useSettingsStore();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const ref = useRef<HTMLDivElement>(null);

  // Reload model list whenever the active endpoint changes
  useEffect(() => {
    const ep = settings?.default_endpoint ?? "openrouter";
    loadModels(ep);
  }, [settings?.default_endpoint]);

  useEffect(() => {
    const close = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, []);

  const displayed = activeModel.split("/").pop() ?? activeModel;
  const filtered = models
    .filter(
      (m) =>
        !query ||
        m.id.toLowerCase().includes(query.toLowerCase()) ||
        m.name.toLowerCase().includes(query.toLowerCase())
    )
    .sort((a, b) => {
      // Custom models first, then alphabetical by id
      if (!!a.is_custom !== !!b.is_custom) return a.is_custom ? -1 : 1;
      return a.id.localeCompare(b.id);
    });

  return (
    <div ref={ref} className="relative">
      <button
        onClick={() => setOpen((o) => !o)}
        className="flex items-center gap-1 rounded px-2 py-1 text-xs text-gray-400 hover:text-gray-200 hover:bg-surface-3 transition-colors"
      >
        <span className="max-w-[160px] truncate">{displayed}</span>
        <ChevronDown size={12} />
      </button>

      {open && (
        <div className="absolute right-0 top-full mt-1 z-50 w-72 rounded-lg border border-border bg-surface-2 shadow-xl">
          <div className="p-2 border-b border-border">
            <input
              autoFocus
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search models..."
              className="w-full bg-surface-3 rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none"
            />
          </div>
          <ul className="max-h-64 overflow-y-auto py-1">
            {filtered.slice(0, 50).map((m) => (
              <li key={m.id}>
                <button
                  className={`flex w-full items-center gap-1.5 px-3 py-1.5 text-left text-xs hover:bg-surface-3 transition-colors ${
                    m.id === activeModel ? "text-accent" : "text-gray-300"
                  }`}
                  onClick={() => { updateActiveSessionModel(m.id); setOpen(false); }}
                  title={m.id}
                >
                  {m.is_custom && (
                    <Sparkles
                      size={10}
                      className="shrink-0 text-amber-400"
                    />
                  )}
                  <span className="truncate">{m.id}</span>
                </button>
              </li>
            ))}
            {filtered.length === 0 && (
              <li className="px-3 py-2 text-xs text-gray-600">No models found</li>
            )}
          </ul>
        </div>
      )}
    </div>
  );
}
