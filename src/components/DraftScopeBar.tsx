// SPDX-License-Identifier: Apache-2.0
//
// DraftScopeBar — the two switches a not-yet-sent conversation has:
//
//   项目    where this conversation works (a directory), or 独立任务 for none
//   匿名    leave no trace: never persisted, never listed
//
// It only appears while the conversation is still a draft, because those are
// exactly the choices that stop being editable once the first message lands.
//
// The critical property: picking a project here re-scopes THIS blank draft. It
// never opens the project's previous conversation. Choosing where to work and
// choosing which conversation to resume are different acts, and every surface
// that blurred them is what made "选了项目" drop users into old history.
import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Folder, FolderOpen, Check, ChevronDown, EyeOff, MessageSquare, MoreHorizontal, X } from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { folderName, type ProjectGroup } from "../lib/projects";

interface DraftScopeBarProps {
  /** Directory this draft works in; null = standalone task. */
  cwd: string | null;
  anonymous: boolean;
  /** Recently used projects, newest first. */
  projects: ProjectGroup[];
  /** The model control belongs beside the draft scope choices. */
  modelPicker?: ReactNode;
  onPickProject: (cwd: string | null) => void;
  onToggleAnonymous: (anonymous: boolean) => void;
}

export function DraftScopeBar({
  cwd,
  anonymous,
  projects,
  modelPicker,
  onPickProject,
  onToggleAnonymous,
}: DraftScopeBarProps) {
  const [openMenu, setOpenMenu] = useState<"project" | "more" | null>(null);
  const idBase = useId().replace(/:/g, "");
  const projectMenuId = `draft-project-menu-${idBase}`;
  const moreDialogId = `draft-more-dialog-${idBase}`;
  const [menuPosition, setMenuPosition] = useState<{ left: number; top: number } | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const projectButtonRef = useRef<HTMLButtonElement>(null);
  const moreButtonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  const updateMenuPosition = () => {
    const button = openMenu === "more" ? moreButtonRef.current : projectButtonRef.current;
    const rect = button?.getBoundingClientRect();
    if (!rect) return;
    const composerTop = button
      ?.closest<HTMLElement>('[data-testid="message-input-control-row"]')
      ?.getBoundingClientRect().top;
    const menuWidth = 256;
    const gutter = 8;
    const menuHeight = Math.min(
      menuRef.current?.getBoundingClientRect().height || (openMenu === "more" ? 92 : 240),
      window.innerHeight - gutter * 2,
    );
    const maxLeft = Math.max(gutter, window.innerWidth - menuWidth - gutter);
    // The portal belongs above the whole composer card, not merely above its
    // trigger in the bottom toolbar. Anchoring to the trigger lets a taller
    // project menu cover the input row once 44px touch targets are applied.
    const aboveTop = (composerTop ?? rect.top) - menuHeight - 4;
    setMenuPosition({
      left: Math.min(Math.max(gutter, rect.left + 4), maxLeft),
      top: Math.max(gutter, aboveTop),
    });
  };

  const toggleMenu = (kind: "project" | "more") => {
    setOpenMenu((current) => current === kind ? null : kind);
  };

  const closeMenuAndRestoreFocus = (kind: "project" | "more" | null = openMenu) => {
    const trigger = kind === "more" ? moreButtonRef.current : projectButtonRef.current;
    setOpenMenu(null);
    trigger?.focus();
  };

  useLayoutEffect(() => {
    if (!openMenu) return;
    updateMenuPosition();
    window.addEventListener("resize", updateMenuPosition);
    window.addEventListener("scroll", updateMenuPosition, true);
    return () => {
      window.removeEventListener("resize", updateMenuPosition);
      window.removeEventListener("scroll", updateMenuPosition, true);
    };
  }, [openMenu]);

  useEffect(() => {
    if (!openMenu) return;
    const focusFrame = requestAnimationFrame(() => {
      menuRef.current?.querySelector<HTMLElement>("button")?.focus();
    });
    const onClick = (e: MouseEvent) => {
      const target = e.target as Node;
      if (rootRef.current?.contains(target) || menuRef.current?.contains(target)) return;
      setOpenMenu(null);
    };
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      closeMenuAndRestoreFocus();
    };
    document.addEventListener("mousedown", onClick);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      cancelAnimationFrame(focusFrame);
      document.removeEventListener("mousedown", onClick);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [openMenu]);

  const browse = async () => {
    closeMenuAndRestoreFocus("project");
    const dir = await openDialog({ directory: true, title: "选择项目目录" });
    if (dir) onPickProject(dir as string);
  };

  const handleProjectMenuKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Tab") {
      event.preventDefault();
      closeMenuAndRestoreFocus("project");
      return;
    }
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    const items = Array.from(
      menuRef.current?.querySelectorAll<HTMLElement>('[role="menuitemradio"], [role="menuitem"]') ?? [],
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
  };

  const label = cwd ? folderName(cwd) : "独立任务";
  // A directory picked via 浏览目录… has no sessions yet, so it isn't in the
  // recent list. Show it anyway — the menu must always reflect the current
  // choice, otherwise nothing appears selected.
  const options =
    cwd && !projects.some((project) => project.cwd === cwd)
      ? [{ cwd, name: folderName(cwd) || cwd, sessions: [], updatedAt: 0 }, ...projects]
      : projects;

  return (
    <div
      className="relative flex min-w-0 max-w-full flex-1 items-center gap-1"
      ref={rootRef}
    >
      <button
        ref={projectButtonRef}
        type="button"
        onClick={() => toggleMenu("project")}
        aria-label={`选择项目：${label}`}
        aria-expanded={openMenu === "project"}
        aria-controls={projectMenuId}
        aria-haspopup="menu"
        title={cwd ?? "不使用项目，只做一个独立任务"}
        className="flex min-h-[44px] min-w-0 max-w-[132px] shrink items-center gap-1.5 rounded-lg px-2 text-xs text-gray-400 transition-colors hover:bg-surface-3 hover:text-gray-100 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent sm:max-w-[220px] lg:min-h-[36px]"
      >
        {cwd ? (
          <Folder size={11} className="shrink-0 text-accent" />
        ) : (
          <MessageSquare size={11} className="shrink-0 text-gray-500" />
        )}
        <span className="truncate">{label}</span>
        <ChevronDown size={11} className="shrink-0 text-gray-600" />
      </button>

      {modelPicker && (
        <>
          <span aria-hidden="true" className="h-4 w-px shrink-0 bg-border/70" />
          <div data-testid="draft-model-picker" className="min-w-0 shrink">
            {modelPicker}
          </div>
        </>
      )}

      <span className="flex-1" />
      {anonymous ? (
        <div
          role="status"
          aria-label="匿名会话已开启"
          className="flex min-h-[44px] shrink-0 items-center gap-1 rounded-lg bg-status-warning-soft pl-2 text-xs font-medium text-status-warning lg:min-h-[36px]"
          title="匿名会话：聊完不留记录"
        >
          <EyeOff size={13} aria-hidden="true" />
          <span>匿名</span>
          <button
            type="button"
            onClick={() => onToggleAnonymous(false)}
            aria-label="关闭匿名会话"
            className="flex h-[44px] w-[44px] items-center justify-center rounded-lg hover:bg-status-warning/10 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent lg:h-[36px] lg:w-[36px]"
          >
            <X size={13} aria-hidden="true" />
          </button>
        </div>
      ) : (
        <button
          ref={moreButtonRef}
          type="button"
          onClick={() => toggleMenu("more")}
          aria-label="更多选项"
          aria-expanded={openMenu === "more"}
          aria-controls={moreDialogId}
          aria-haspopup="dialog"
          title="更多会话设置"
          className="flex h-[44px] w-[44px] shrink-0 items-center justify-center rounded-lg text-gray-500 transition-colors hover:bg-surface-3 hover:text-gray-200 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent lg:h-[36px] lg:w-[36px]"
        >
          <MoreHorizontal size={15} aria-hidden="true" />
        </button>
      )}

      {openMenu === "project" && typeof document !== "undefined" && createPortal(
        <div
          ref={menuRef}
          id={projectMenuId}
          role="menu"
          aria-label="项目选择"
          onKeyDown={handleProjectMenuKeyDown}
          style={menuPosition ? { left: menuPosition.left, top: menuPosition.top } : undefined}
          className="fixed z-[100] max-h-[calc(100vh-1rem)] w-64 overflow-y-auto rounded-lg border border-border bg-surface-2 py-1 shadow-2xl"
        >
          <p role="presentation" className="px-3 py-1 text-[11px] font-medium tracking-wide text-gray-600">在哪里干活</p>
          <button
            type="button"
            role="menuitemradio"
            aria-checked={cwd === null}
            onClick={() => {
              onPickProject(null);
              closeMenuAndRestoreFocus("project");
            }}
            className="flex min-h-[44px] w-full items-center gap-2 px-3 text-left text-[11px] text-gray-300 transition-colors hover:bg-surface-3 focus:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-accent lg:min-h-[36px]"
          >
            <MessageSquare size={11} className="shrink-0 text-gray-500" />
            <span className="flex-1 truncate">独立任务（不使用项目）</span>
            {cwd === null && <Check size={11} className="shrink-0 text-accent" />}
          </button>
          {options.length > 0 && (
            <>
              <p role="presentation" className="mt-1 border-t border-border px-3 pb-0.5 pt-1.5 text-[11px] font-medium tracking-wide text-gray-600">
                最近项目
              </p>
              {options.map((project) => (
                <button
                  key={project.cwd}
                  type="button"
                  role="menuitemradio"
                  aria-checked={cwd === project.cwd}
                  onClick={() => {
                    onPickProject(project.cwd);
                    closeMenuAndRestoreFocus("project");
                  }}
                  title={project.cwd}
                  className="flex min-h-[44px] w-full items-center gap-2 px-3 text-left text-[11px] text-gray-300 transition-colors hover:bg-surface-3 focus:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-accent lg:min-h-[36px]"
                >
                  <Folder size={11} className="shrink-0 text-gray-500" />
                  <span className="flex-1 truncate">{project.name}</span>
                  {cwd === project.cwd && <Check size={11} className="shrink-0 text-accent" />}
                </button>
              ))}
            </>
          )}
          <button
            type="button"
            role="menuitem"
            onClick={() => void browse()}
            className="mt-1 flex min-h-[44px] w-full items-center gap-2 border-t border-border px-3 text-left text-[11px] text-gray-300 transition-colors hover:bg-surface-3 focus:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-accent lg:min-h-[36px]"
          >
            <FolderOpen size={11} className="shrink-0 text-gray-500" />
            <span className="flex-1">浏览目录…</span>
          </button>
        </div>,
        document.body,
      )}
      {openMenu === "more" && typeof document !== "undefined" && createPortal(
        <div
          ref={menuRef}
          id={moreDialogId}
          role="dialog"
          aria-label="会话设置"
          onKeyDown={(event) => {
            if (event.key !== "Tab") return;
            event.preventDefault();
            closeMenuAndRestoreFocus("more");
          }}
          style={menuPosition ? { left: menuPosition.left, top: menuPosition.top } : undefined}
          className="fixed z-[100] max-h-[calc(100vh-1rem)] w-64 overflow-y-auto rounded-lg border border-border bg-surface-2 p-1 shadow-2xl"
        >
          <button
            type="button"
            role="switch"
            aria-label="匿名会话"
            aria-checked="false"
            onClick={() => {
              closeMenuAndRestoreFocus("more");
              onToggleAnonymous(true);
            }}
            className="flex min-h-[44px] w-full items-center gap-2 rounded-md px-2 text-left text-xs text-gray-300 transition-colors hover:bg-surface-3 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent lg:min-h-[36px]"
          >
            <EyeOff size={14} className="shrink-0 text-gray-500" aria-hidden="true" />
            <span className="flex-1">匿名会话</span>
            <span className="text-[11px] text-gray-500">不留记录</span>
          </button>
        </div>,
        document.body,
      )}
    </div>
  );
}
