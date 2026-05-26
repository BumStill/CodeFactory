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
    <div className="border-t border-border bg-surface-1">
      <button
        onClick={() => setExpanded((e) => !e)}
        className="w-full flex items-center gap-2 px-4 py-1.5 text-[11px] text-gray-500 hover:text-gray-300 hover:bg-surface-2 transition-colors"
      >
        <Clock size={11} className="text-accent" />
        <span>
          已排队 <span className="text-accent font-medium">{queue.length}</span> 条，将在当前流式完成后依次发送
        </span>
        {expanded ? <ChevronDown size={11} className="ml-auto" /> : <ChevronUp size={11} className="ml-auto" />}
      </button>
      {expanded && (
        <ul className="px-4 pb-2 space-y-1">
          {queue.map((q, i) => (
            <li
              key={q.id}
              className="group flex items-start gap-2 p-2 rounded bg-surface-2 border border-border text-xs"
            >
              <span className="text-[10px] text-gray-600 font-mono shrink-0 mt-0.5">#{i + 1}</span>
              <p className="flex-1 text-gray-300 line-clamp-3 leading-snug whitespace-pre-wrap break-words">
                {q.content}
              </p>
              <button
                onClick={() => onRemove(q.id)}
                className="shrink-0 opacity-0 group-hover:opacity-100 transition-opacity p-0.5 rounded text-gray-500 hover:text-red-400 hover:bg-surface-3"
                title="移除"
              >
                <X size={11} />
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
