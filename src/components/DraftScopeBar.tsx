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
import { useEffect, useRef, useState } from "react";
import { Folder, FolderOpen, Check, ChevronDown, EyeOff, MessageSquare } from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { folderName, type ProjectGroup } from "../lib/projects";

interface DraftScopeBarProps {
  /** Directory this draft works in; null = standalone task. */
  cwd: string | null;
  anonymous: boolean;
  /** Recently used projects, newest first. */
  projects: ProjectGroup[];
  onPickProject: (cwd: string | null) => void;
  onToggleAnonymous: (anonymous: boolean) => void;
}

export function DraftScopeBar({
  cwd,
  anonymous,
  projects,
  onPickProject,
  onToggleAnonymous,
}: DraftScopeBarProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!menuOpen) return;
    const onClick = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) setMenuOpen(false);
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
    <div className="relative flex items-center gap-2 px-1 pb-1.5" ref={menuRef}>
      <button
        type="button"
        onClick={() => setMenuOpen((v) => !v)}
        aria-label="选择项目"
        aria-expanded={menuOpen}
        title={cwd ?? "不使用项目，只做一个独立任务"}
        className="flex max-w-[260px] items-center gap-1.5 rounded-full border border-border bg-surface-2 px-2.5 py-1 text-[11px] text-gray-300 transition-colors hover:border-accent/40 hover:text-gray-100"
      >
        {cwd ? (
          <Folder size={11} className="shrink-0 text-accent" />
        ) : (
          <MessageSquare size={11} className="shrink-0 text-gray-500" />
        )}
        <span className="truncate">{label}</span>
        <ChevronDown size={11} className="shrink-0 text-gray-600" />
      </button>

      <button
        type="button"
        onClick={() => onToggleAnonymous(!anonymous)}
        aria-pressed={anonymous}
        title="匿名：这次对话不留任何记录"
        className={`flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-[11px] transition-colors ${
          anonymous
            ? "border-accent/50 bg-accent/10 text-accent"
            : "border-border bg-surface-2 text-gray-500 hover:text-gray-300"
        }`}
      >
        <EyeOff size={11} />
        匿名
      </button>

      <span className="truncate text-[11px] text-gray-600">
        {anonymous
          ? "聊完不留记录"
          : cwd
            ? "新会话 · 不会打开这个项目的历史对话"
            : "没选项目，不会碰任何代码"}
      </span>

      {menuOpen && (
        <div
          role="menu"
          aria-label="项目选择"
          className="absolute bottom-full left-1 z-50 mb-1 w-64 overflow-hidden rounded-lg border border-border bg-surface-2 py-1 shadow-xl"
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
        </div>
      )}
    </div>
  );
}
