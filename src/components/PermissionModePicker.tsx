// SPDX-License-Identifier: Apache-2.0
import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Check, ShieldCheck } from "lucide-react";
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
  const menuId = `permission-mode-menu-${useId().replace(/:/g, "")}`;
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [menuPosition, setMenuPosition] = useState({ left: 8, top: 8 });

  const closeAndRestoreFocus = useCallback(() => {
    setOpen(false);
    triggerRef.current?.focus();
  }, []);

  const updateMenuPosition = useCallback(() => {
    const rect = triggerRef.current?.getBoundingClientRect();
    if (!rect || typeof window === "undefined") return;
    const gutter = 8;
    const menuWidth = Math.min(288, window.innerWidth - gutter * 2);
    const menuHeight = 184;
    setMenuPosition({
      left: Math.max(gutter, Math.min(rect.right - menuWidth, window.innerWidth - menuWidth - gutter)),
      top: Math.max(gutter, rect.top - menuHeight - 4),
    });
  }, []);

  useLayoutEffect(() => {
    if (!open) return;
    updateMenuPosition();
    window.addEventListener("resize", updateMenuPosition);
    window.addEventListener("scroll", updateMenuPosition, true);
    return () => {
      window.removeEventListener("resize", updateMenuPosition);
      window.removeEventListener("scroll", updateMenuPosition, true);
    };
  }, [open, updateMenuPosition]);

  useEffect(() => {
    if (!open) return;
    const focusFrame = requestAnimationFrame(() => {
      menuRef.current
        ?.querySelector<HTMLElement>('[role="menuitemradio"][aria-checked="true"]')
        ?.focus();
    });
    const onMouseDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (triggerRef.current?.contains(target) || menuRef.current?.contains(target)) return;
      setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      closeAndRestoreFocus();
    };
    document.addEventListener("mousedown", onMouseDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      cancelAnimationFrame(focusFrame);
      document.removeEventListener("mousedown", onMouseDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [closeAndRestoreFocus, open]);

  if (!activeSession || activeSession.kind === "anonymous") return null;
  const mode = activeSession.permission_mode ?? "standard";
  const current = OPTIONS.find((option) => option.id === mode) ?? OPTIONS[1];
  const description = `当前为${current.label}模式：${current.description}。更改将在下一次权限判断生效。`;
  const visibleRisk = mode !== "standard";

  const selectMode = (next: PermissionMode) => {
    setOpen(false);
    if (onChangeForAcceptance) {
      onChangeForAcceptance(next);
    } else {
      void update(next);
    }
    requestAnimationFrame(() => triggerRef.current?.focus());
  };

  return (
    <>
      <span id={descriptionId} className="sr-only">{description}</span>
      <button
        ref={triggerRef}
        id="workspace-permission-mode"
        type="button"
        onClick={() => setOpen((value) => !value)}
        aria-label={`会话权限：${current.label}`}
        aria-describedby={descriptionId}
        aria-expanded={open}
        aria-controls={menuId}
        aria-haspopup="menu"
        title={`会话权限：${current.description}；下一次权限判断生效`}
        className={`flex min-h-11 min-w-11 shrink-0 items-center justify-center gap-1 rounded-lg px-2 text-xs transition-colors focus:outline-none focus-visible:ring-2 focus-visible:ring-accent lg:min-h-9 lg:min-w-9 ${
          mode === "trusted"
            ? "bg-status-warning-soft text-status-warning hover:brightness-95"
            : mode === "safe"
              ? "bg-status-progress-soft text-status-progress hover:brightness-95"
              : "text-gray-500 hover:bg-surface-3 hover:text-gray-200"
        }`}
      >
        <ShieldCheck size={14} aria-hidden="true" />
        {visibleRisk && <span>{current.label}</span>}
      </button>
      {open && typeof document !== "undefined" && createPortal(
        <div
          ref={menuRef}
          id={menuId}
          role="menu"
          aria-label="选择会话权限"
          onKeyDown={(event) => {
            if (event.key === "Tab") {
              event.preventDefault();
              closeAndRestoreFocus();
              return;
            }
            if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
            event.preventDefault();
            const items = Array.from(
              menuRef.current?.querySelectorAll<HTMLElement>('[role="menuitemradio"]') ?? [],
            );
            if (items.length === 0) return;
            const currentIndex = items.indexOf(document.activeElement as HTMLElement);
            const nextIndex = event.key === "Home"
              ? 0
              : event.key === "End"
                ? items.length - 1
                : event.key === "ArrowDown"
                  ? (currentIndex + 1 + items.length) % items.length
                  : (currentIndex - 1 + items.length) % items.length;
            items[nextIndex]?.focus();
          }}
          style={{ left: menuPosition.left, top: menuPosition.top }}
          className="fixed z-[100] w-72 max-w-[calc(100vw-1rem)] rounded-lg border border-border bg-surface-2 p-1 shadow-2xl"
        >
          <p className="px-2 pb-1 pt-1.5 text-[11px] font-medium text-gray-500">下一次权限判断生效</p>
          {OPTIONS.map((option) => (
            <button
              key={option.id}
              type="button"
              role="menuitemradio"
              aria-checked={mode === option.id}
              onClick={() => selectMode(option.id)}
              className="flex min-h-11 w-full items-start gap-2 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-surface-3 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent lg:min-h-9"
            >
              <span className="mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center">
                {mode === option.id && <Check size={13} className="text-accent" aria-hidden="true" />}
              </span>
              <span className="min-w-0">
                <span className="block text-xs text-gray-200">{option.label}</span>
                <span className="block text-[11px] leading-4 text-gray-500">{option.description}</span>
              </span>
            </button>
          ))}
        </div>,
        document.body,
      )}
    </>
  );
}
