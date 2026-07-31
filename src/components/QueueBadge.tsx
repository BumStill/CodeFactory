// SPDX-License-Identifier: Apache-2.0
//
// Shows messages queued while the assistant is streaming. The queue drains
// FIFO as soon as the current stream lands in a terminal state. Each row
// has a delete button so the user can cancel a queued item before it fires
// (the common case: "wait no, the AI is going to answer that").

import { useState } from "react";
import { Clock, X, ChevronUp, ChevronDown } from "lucide-react";
import type { QueuedMessage } from "../stores/chat";

interface Props {
  queue: QueuedMessage[];
  onRemove: (id: string) => void;
}

export function QueueBadge({ queue, onRemove }: Props) {
  const [expanded, setExpanded] = useState(false);

  if (queue.length === 0) return null;

  return (
    <div className="px-3 pt-2">
      <button
        type="button"
        onClick={() => setExpanded((e) => !e)}
        aria-label={`${expanded ? "收起" : "查看"} ${queue.length} 条待发消息`}
        aria-expanded={expanded}
        className="flex min-h-8 max-w-full items-center gap-2 rounded-lg bg-status-progress-soft px-2.5 text-[13px] text-status-progress transition-colors hover:brightness-95"
      >
        <Clock size={13} aria-hidden="true" />
        <span className="truncate">
          {queue.length} 条待发消息 · 当前执行结束后发送
        </span>
        {expanded
          ? <ChevronUp size={13} aria-hidden="true" className="ml-auto shrink-0" />
          : <ChevronDown size={13} aria-hidden="true" className="ml-auto shrink-0" />}
      </button>
      {expanded && (
        <ul className="mt-2 space-y-1.5">
          {queue.map((q, i) => (
            <li
              key={q.id}
              className="group flex items-start gap-2 rounded-lg border border-border/70 bg-surface-1 p-2.5 text-[13px]"
            >
              <span className="mt-0.5 shrink-0 font-mono text-[11px] text-gray-600">#{i + 1}</span>
              <p className="flex-1 text-gray-300 line-clamp-3 leading-snug whitespace-pre-wrap break-words">
                {q.content}
              </p>
              <button
                type="button"
                onClick={() => onRemove(q.id)}
                className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg text-gray-500 opacity-70 transition-colors hover:bg-status-danger-soft hover:text-status-danger focus:opacity-100 group-hover:opacity-100"
                title={`移除第 ${i + 1} 条待发消息`}
                aria-label={`移除第 ${i + 1} 条待发消息`}
              >
                <X size={14} />
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
