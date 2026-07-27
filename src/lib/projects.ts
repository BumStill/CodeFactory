// SPDX-License-Identifier: Apache-2.0
//
// Projects, derived from sessions.
//
// A project is not (yet) a stored entity — it's the set of sessions that share
// a working directory. That is enough to give the UI the thing users actually
// have in their heads: "the place I work", with a history of conversations
// inside it. Deriving keeps identity in one place (`cwd`) instead of spreading
// a second notion of project across the store, and needs no migration.
//
// The one thing this file must never do is blur the line the app got wrong
// before: a project groups sessions, it is NOT itself a session. Selecting a
// project is choosing where to work; opening a conversation is a separate act.
import type { Session } from "./tauri";

export interface ProjectGroup {
  /** Absolute directory — the identity of a project today. */
  cwd: string;
  /** Trailing folder name, for display. */
  name: string;
  /** Conversations in this project, newest first. */
  sessions: Session[];
  /** `updated_at` of the newest conversation, for ordering projects. */
  updatedAt: number;
}

export interface SessionGrouping {
  projects: ProjectGroup[];
  /** Conversations with no project — standalone tasks. */
  standalone: Session[];
}

/** Display name for a directory: its last path segment, either separator. */
export function folderName(cwd: string | null | undefined): string {
  if (!cwd) return "";
  return cwd.split(/[/\\]/).filter(Boolean).pop() ?? cwd;
}

/** True when this session works inside a real project directory. */
export function isProjectSession(session: Session): boolean {
  return session.kind !== "quick" && session.kind !== "anonymous" && Boolean(session.cwd);
}

/**
 * Split a flat session list into projects (grouped by directory, newest first)
 * and standalone tasks. Anonymous sessions are never listed — they leave no
 * trace by definition.
 */
export function groupSessionsByProject(sessions: Session[]): SessionGrouping {
  const byCwd = new Map<string, Session[]>();
  const standalone: Session[] = [];

  for (const session of sessions) {
    if (session.kind === "anonymous") continue;
    if (isProjectSession(session)) {
      const bucket = byCwd.get(session.cwd);
      if (bucket) bucket.push(session);
      else byCwd.set(session.cwd, [session]);
    } else {
      standalone.push(session);
    }
  }

  const byRecency = (a: Session, b: Session) => b.updated_at - a.updated_at;
  const projects: ProjectGroup[] = [...byCwd.entries()]
    .map(([cwd, group]) => {
      const ordered = [...group].sort(byRecency);
      return {
        cwd,
        name: folderName(cwd) || cwd,
        sessions: ordered,
        updatedAt: ordered[0]?.updated_at ?? 0,
      };
    })
    .sort((a, b) => b.updatedAt - a.updatedAt);

  return { projects, standalone: [...standalone].sort(byRecency) };
}

/** The most recently touched projects — for the draft's project picker. */
export function recentProjects(sessions: Session[], limit = 8): ProjectGroup[] {
  return groupSessionsByProject(sessions).projects.slice(0, limit);
}
