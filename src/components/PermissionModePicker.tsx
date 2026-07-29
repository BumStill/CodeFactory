// SPDX-License-Identifier: Apache-2.0
import { ShieldCheck } from "lucide-react";
import { useChatStore } from "../stores/chat";
import type { PermissionMode } from "../lib/tauri";

const OPTIONS: Array<{ id: PermissionMode; label: string; description: string }> = [
  { id: "safe", label: "安全", description: "读取自动允许，写入和命令先确认" },
  { id: "standard", label: "标准", description: "常规文件操作自动允许，命令先确认" },
  { id: "trusted", label: "信任", description: "普通命令也可自动执行，高风险仍拦截" },
];

export function PermissionModePicker({
  onChangeForAcceptance,
}: {
  onChangeForAcceptance?: (mode: PermissionMode) => void;
} = {}) {
  const activeSession = useChatStore((s) => s.activeSession);
  const update = useChatStore((s) => s.updateActiveSessionPermissionMode);
  if (!activeSession || activeSession.kind === "anonymous") return null;
  const mode = activeSession.permission_mode ?? "standard";
  const current = OPTIONS.find((option) => option.id === mode) ?? OPTIONS[1];
  return (
    <label className="flex min-w-0 shrink items-center gap-1 rounded px-1.5 py-1 text-xs text-gray-400 hover:bg-surface-3 hover:text-gray-200" title={`会话权限：${current.description}`}>
      <ShieldCheck size={12} />
      <span className="sr-only">会话权限</span>
      <span className="hidden lg:inline">会话权限</span>
      <select
        aria-label="会话权限"
        value={mode}
        onChange={(event) => {
          const next = event.target.value as PermissionMode;
          if (onChangeForAcceptance) {
            onChangeForAcceptance(next);
          } else {
            void update(next);
          }
        }}
        className="rounded border border-transparent bg-transparent text-xs text-gray-300 outline-none hover:border-border focus:border-accent/50"
      >
        {OPTIONS.map((option) => (
          <option key={option.id} value={option.id}>{option.label}</option>
        ))}
      </select>
    </label>
  );
}
