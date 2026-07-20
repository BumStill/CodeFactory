// SPDX-License-Identifier: Apache-2.0
//
// Secure secret prompt — the ONE step the user performs in conversational
// git setup (the agent does everything else). The value is masked, submitted
// directly to the provide_secret command by the caller, and never enters
// chat content, stream events, or the DB.

import { useState } from "react";
import { KeyRound } from "lucide-react";
import type { PendingSecret } from "../stores/chatEvents";

export function SecretPromptModal({
  request,
  onSubmit,
  onCancel,
}: {
  request: PendingSecret;
  onSubmit: (value: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState("");
  const canSubmit = value.trim().length > 0;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div className="w-full max-w-md rounded-lg border border-surface-3 bg-surface-1 p-5 shadow-xl space-y-4">
        <div className="flex items-center gap-2 text-sm font-medium text-gray-200">
          <KeyRound size={16} className="text-sky-400" />
          安全输入
        </div>
        <p className="text-sm text-gray-300">{request.purpose}</p>
        {request.hint && <p className="text-xs text-gray-500">{request.hint}</p>}
        <div className="space-y-1.5">
          <label htmlFor="secret-input" className="block text-xs text-gray-400">
            访问令牌
          </label>
          <input
            id="secret-input"
            type="password"
            autoFocus
            autoComplete="off"
            value={value}
            onChange={(e) => setValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && canSubmit) onSubmit(value.trim());
              if (e.key === "Escape") onCancel();
            }}
            className="w-full rounded border border-surface-3 bg-surface-2 px-3 py-2 text-sm text-gray-200 focus:border-sky-500 focus:outline-none"
            placeholder="粘贴令牌"
          />
          <p className="text-[11px] text-gray-500">
            该值直接写入系统钥匙串,不会出现在对话、消息记录或日志中。
          </p>
        </div>
        <div className="flex justify-end gap-2">
          <button
            type="button"
            onClick={onCancel}
            className="rounded border border-surface-3 px-3 py-1.5 text-sm text-gray-300 hover:bg-surface-2"
          >
            取消
          </button>
          <button
            type="button"
            disabled={!canSubmit}
            onClick={() => onSubmit(value.trim())}
            className="rounded bg-sky-600 px-3 py-1.5 text-sm text-white disabled:opacity-40 hover:bg-sky-500"
          >
            保存并验证
          </button>
        </div>
      </div>
    </div>
  );
}
