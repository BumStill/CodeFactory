// SPDX-License-Identifier: Apache-2.0
import { useId } from "react";
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
  const descriptionId = `permission-mode-description-${useId().replace(/:/g, "")}`;
  if (!activeSession || activeSession.kind === "anonymous") return null;
  const mode = activeSession.permission_mode ?? "standard";
  const current = OPTIONS.find((option) => option.id === mode) ?? OPTIONS[1];
  return (
    <label
      className="flex min-h-11 min-w-0 shrink items-center gap-1 rounded-lg px-1 text-xs text-gray-400 hover:bg-surface-3 hover:text-gray-200 lg:min-h-9"
      title={`会话权限：${current.description}；下一次权限判断生效`}
    >
      <ShieldCheck size={14} aria-hidden="true" />
      <span className="sr-only">会话权限</span>
      <span className="hidden lg:inline">会话权限</span>
      <span id={descriptionId} className="sr-only">
        当前为{current.label}模式：{current.description}。更改将在下一次权限判断生效。
      </span>
      <select
        id="workspace-permission-mode"
        aria-label="会话权限"
        aria-describedby={descriptionId}
        value={mode}
        onChange={(event) => {
          const next = event.target.value as PermissionMode;
          if (onChangeForAcceptance) {
            onChangeForAcceptance(next);
          } else {
            void update(next);
          }
        }}
        className="min-h-11 rounded-lg border border-transparent bg-transparent px-1 text-xs text-gray-300 outline-none hover:border-border focus:border-accent/50 focus-visible:ring-2 focus-visible:ring-accent/60 lg:min-h-9"
      >
        {OPTIONS.map((option) => (
          <option key={option.id} value={option.id}>{option.label}</option>
        ))}
      </select>
    </label>
  );
}
