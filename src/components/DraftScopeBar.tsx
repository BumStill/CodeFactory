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
import { useEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Folder, FolderOpen, Check, ChevronDown, EyeOff, MessageSquare } from "lucide-react";
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
  const [menuOpen, setMenuOpen] = useState(false);
  const [menuPosition, setMenuPosition] = useState<{ left: number; top: number } | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  const updateMenuPosition = () => {
    const rect = buttonRef.current?.getBoundingClientRect();
    if (!rect) return;
    const menuWidth = 256;
    const menuHeight = 180;
    const gutter = 8;
    const maxLeft = Math.max(gutter, window.innerWidth - menuWidth - gutter);
    const aboveTop = rect.top - menuHeight - 4;
    setMenuPosition({
      left: Math.min(Math.max(gutter, rect.left + 4), maxLeft),
      top: Math.max(gutter, aboveTop),
    });
  };

  const toggleMenu = () => {
    setMenuOpen((open) => {
      if (!open) updateMenuPosition();
      return !open;
    });
  };

  useEffect(() => {
    if (!menuOpen) return;
    updateMenuPosition();
    window.addEventListener("resize", updateMenuPosition);
    window.addEventListener("scroll", updateMenuPosition, true);
    return () => {
      window.removeEventListener("resize", updateMenuPosition);
      window.removeEventListener("scroll", updateMenuPosition, true);
    };
  }, [menuOpen]);

  useEffect(() => {
    if (!menuOpen) return;
    const onClick = (e: MouseEvent) => {
      const target = e.target as Node;
      if (rootRef.current?.contains(target) || menuRef.current?.contains(target)) return;
      setMenuOpen(false);
    };
    document.addEventListener("mousedown", onClick);
    return () => document.removeEventListener("mousedown", onClick);
  }, [menuOpen]);

  const browse = async () => {
    const dir = await openDialog({ directory: true, title: "选择项目目录" });
    setMenuOpen(false);
    if (dir) onPickProject(dir as string);
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
      className="relative flex flex-wrap items-center gap-1.5 border-b border-border/60 bg-surface-1/30 px-3 py-2"
      ref={rootRef}
    >
      <span className="mr-0.5 shrink-0 text-[10px] font-medium uppercase tracking-[0.08em] text-gray-600">
        新会话
      </span>
      <button
        ref={buttonRef}
        type="button"
        onClick={toggleMenu}
        aria-label="选择项目"
        aria-expanded={menuOpen}
        title={cwd ?? "不使用项目，只做一个独立任务"}
        className="flex min-h-8 max-w-[240px] items-center gap-1.5 rounded-lg border border-border bg-surface-2 px-2.5 text-[11px] text-gray-300 transition-colors hover:border-accent/40 hover:bg-surface-3 hover:text-gray-100"
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
          <span aria-hidden="true" className="mx-0.5 h-4 w-px shrink-0 bg-border/70" />
          <div data-testid="draft-model-picker" className="min-w-0 max-w-full">
            {modelPicker}
          </div>
        </>
      )}

      <button
        type="button"
        onClick={() => onToggleAnonymous(!anonymous)}
        aria-pressed={anonymous}
        title="匿名：这次对话不留任何记录"
        className={`flex min-h-8 items-center gap-1.5 rounded-lg border px-2.5 text-[11px] transition-colors ${
          anonymous
            ? "border-accent/50 bg-accent/10 text-accent"
            : "border-border bg-surface-2 text-gray-500 hover:bg-surface-3 hover:text-gray-300"
        }`}
      >
        <EyeOff size={11} />
        匿名
      </button>

      <span className="min-w-[160px] flex-1 truncate px-1 text-[11px] text-gray-600">
        {anonymous
          ? "聊完不留记录"
          : cwd
            ? "不会打开这个项目的历史对话"
            : "没选项目，不会碰任何代码"}
      </span>

      {menuOpen && typeof document !== "undefined" && createPortal(
        <div
          ref={menuRef}
          role="menu"
          aria-label="项目选择"
          style={menuPosition ? { left: menuPosition.left, top: menuPosition.top } : undefined}
          className="fixed z-[100] w-64 overflow-hidden rounded-lg border border-border bg-surface-2 py-1 shadow-2xl"
        >
          <p className="px-3 py-1 text-[11px] font-medium tracking-wide text-gray-600">在哪里干活</p>
          <button
            type="button"
            onClick={() => {
              onPickProject(null);
              setMenuOpen(false);
            }}
            className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-[11px] text-gray-300 transition-colors hover:bg-surface-3"
          >
            <MessageSquare size={11} className="shrink-0 text-gray-500" />
            <span className="flex-1 truncate">独立任务（不使用项目）</span>
            {cwd === null && <Check size={11} className="shrink-0 text-accent" />}
          </button>
          {options.length > 0 && (
            <>
              <p className="mt-1 border-t border-border px-3 pb-0.5 pt-1.5 text-[11px] font-medium tracking-wide text-gray-600">
                最近项目
              </p>
              {options.map((project) => (
                <button
                  key={project.cwd}
                  type="button"
                  onClick={() => {
                    onPickProject(project.cwd);
                    setMenuOpen(false);
                  }}
                  title={project.cwd}
                  className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-[11px] text-gray-300 transition-colors hover:bg-surface-3"
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
            onClick={() => void browse()}
            className="mt-1 flex w-full items-center gap-2 border-t border-border px-3 py-1.5 text-left text-[11px] text-gray-300 transition-colors hover:bg-surface-3"
          >
            <FolderOpen size={11} className="shrink-0 text-gray-500" />
            <span className="flex-1">浏览目录…</span>
          </button>
        </div>,
        document.body,
      )}
    </div>
  );
}
