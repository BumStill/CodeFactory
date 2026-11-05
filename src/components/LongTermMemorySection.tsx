// SPDX-License-Identifier: Apache-2.0
import { useCallback, useEffect, useState } from "react";
import { Brain } from "lucide-react";
import {
  deleteLongTermMemory,
  editLongTermMemory,
  listLongTermMemories,
  setLongTermMemoryEnabled,
  type LongTermMemory,
  type LongTermMemoryOverview,
} from "../lib/longTermMemory";

const KIND_LABELS: Record<LongTermMemory["kind"], string> = {
  preference: "偏好",
  project_fact: "项目事实",
  decision: "决定",
};

function scopeLabel(memory: LongTermMemory): string {
  return memory.scope === "user" ? "所有项目" : memory.repo_label || "当前项目";
}

function MemoryRow({ memory, onChanged, onError }: {
  memory: LongTermMemory;
  onChanged: () => Promise<void>;
  onError: (message: string) => void;
}) {
  const [draft, setDraft] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [showSource, setShowSource] = useState(false);
  const active = memory.status === "active";

  const run = async (action: () => Promise<unknown>) => {
    try {
      await action();
      await onChanged();
    } catch (error) {
      onError(String(error));
    }
  };

  return (
    <li className={`space-y-2 rounded-xl border border-border bg-surface-1 p-3 ${active ? "" : "opacity-60"}`}>
      {draft === null ? (
        <p className="whitespace-pre-wrap break-words text-body text-gray-200">{memory.content}</p>
      ) : (
        <textarea
          aria-label="修改记忆内容"
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          rows={3}
          className="w-full rounded border border-border bg-surface-0 px-2 py-1.5 text-body text-gray-200"
        />
      )}
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-label text-gray-500">
        <span>{KIND_LABELS[memory.kind]}</span>
        <span>{scopeLabel(memory)}</span>
        <span>{new Date(memory.created_at).toLocaleDateString("zh-CN")}</span>
        {!active && <span>已被新内容取代</span>}
        <span className="ml-auto flex flex-wrap items-center gap-2">
          <button type="button" className="min-h-8 px-1 hover:text-gray-200" onClick={() => setShowSource((shown) => !shown)}>
            {showSource ? "收起来源" : "查看来源"}
          </button>
          {active && draft === null && (
            <button type="button" className="min-h-8 px-1 hover:text-gray-200" onClick={() => setDraft(memory.content)}>修改</button>
          )}
          {draft !== null && (
            <>
              <button type="button" className="min-h-8 px-1 hover:text-gray-200" onClick={() => void run(async () => { await editLongTermMemory(memory.id, draft); setDraft(null); })}>保存</button>
              <button type="button" className="min-h-8 px-1 hover:text-gray-200" onClick={() => setDraft(null)}>取消</button>
            </>
          )}
          {confirming ? (
            <>
              <button type="button" className="min-h-8 px-1 text-red-700 dark:text-red-300" onClick={() => void run(() => deleteLongTermMemory(memory.id))}>确认删除</button>
              <button type="button" className="min-h-8 px-1 hover:text-gray-200" onClick={() => setConfirming(false)}>不删</button>
            </>
          ) : (
            <button type="button" className="min-h-8 px-1 hover:text-gray-200" onClick={() => setConfirming(true)}>删除</button>
          )}
        </span>
      </div>
      {showSource && (
        <p className="whitespace-pre-wrap break-words rounded border border-border bg-surface-0 px-2 py-1.5 text-label text-gray-400">
          {memory.source_text ?? "原来的会话已经删除，看不到原话了。"}
        </p>
      )}
    </li>
  );
}

/** M235：用户让 CodeFactory 记住的内容，都在这里看得到、改得了、删得掉。 */
export function LongTermMemorySection() {
  const [overview, setOverview] = useState<LongTermMemoryOverview | null>(null);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    setError(null);
    setOverview(await listLongTermMemories());
  }, []);

  useEffect(() => {
    reload().catch((loadError) => setError(String(loadError)));
  }, [reload]);

  const toggle = async (enabled: boolean) => {
    try {
      await setLongTermMemoryEnabled(enabled);
      await reload();
    } catch (toggleError) {
      setError(String(toggleError));
    }
  };

  return (
    <section aria-label="长期记忆" className="max-w-3xl space-y-4">
      <div className="flex items-start justify-between gap-4 rounded-xl border border-border bg-surface-1 p-3">
        <p className="text-label leading-5 text-gray-500">
          在对话里说「记住……」，CodeFactory 会把你的原话存下来，以后的新会话也会参考。项目里的记忆只在这个项目生效。
        </p>
        <label className="flex shrink-0 items-center gap-2 text-label text-gray-300">
          <input
            type="checkbox"
            checked={overview?.enabled ?? false}
            disabled={!overview}
            onChange={(event) => void toggle(event.target.checked)}
          />
          使用长期记忆
        </label>
      </div>

      {error && (
        <div role="alert" className="rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-label text-red-700 dark:text-red-300">
          {error}
        </div>
      )}

      {overview && overview.memories.length === 0 && (
        <div className="flex items-center gap-3 rounded-xl border border-border bg-surface-1 p-4 text-body text-gray-400">
          <Brain size={20} className="text-emerald-400" />
          还没有记忆。
        </div>
      )}

      <ul className="space-y-2">
        {overview?.memories.map((memory) => (
          <MemoryRow key={memory.id} memory={memory} onChanged={reload} onError={setError} />
        ))}
      </ul>
    </section>
  );
}
