// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "vitest";
import { buildSessionRail, folderName, groupSessionsByProject, recentProjects } from "./projects";
import type { Session } from "./tauri";

const mk = (over: Partial<Session> & { id: string }): Session => ({
  title: over.id,
  cwd: "/code/app",
  model_id: "m",
  created_at: 0,
  updated_at: 0,
  total_input_tokens: 0,
  total_output_tokens: 0,
  kind: "project",
  ...over,
});

describe("folderName", () => {
  it("takes the last segment of either separator style", () => {
    expect(folderName("/code/CodeFactory")).toBe("CodeFactory");
    expect(folderName("C:\\code\\ledger")).toBe("ledger");
    expect(folderName("/code/app/")).toBe("app");
    expect(folderName(null)).toBe("");
  });
});

describe("groupSessionsByProject", () => {
  it("groups conversations by directory and orders both levels by recency", () => {
    const { projects } = groupSessionsByProject([
      mk({ id: "a1", cwd: "/code/a", updated_at: 100 }),
      mk({ id: "b1", cwd: "/code/b", updated_at: 300 }),
      mk({ id: "a2", cwd: "/code/a", updated_at: 200 }),
    ]);

    expect(projects.map((p) => p.cwd)).toEqual(["/code/b", "/code/a"]);
    expect(projects[1].sessions.map((s) => s.id)).toEqual(["a2", "a1"]);
    expect(projects[1].updatedAt).toBe(200);
    expect(projects[1].name).toBe("a");
  });

  it("separates standalone tasks from projects", () => {
    const { projects, standalone } = groupSessionsByProject([
      mk({ id: "p", cwd: "/code/a" }),
      mk({ id: "q", cwd: "/home/.codefactory/quick/q", kind: "quick" }),
    ]);

    expect(projects).toHaveLength(1);
    expect(standalone.map((s) => s.id)).toEqual(["q"]);
  });

  it("never lists anonymous conversations", () => {
    const { projects, standalone } = groupSessionsByProject([
      mk({ id: "anon", cwd: "/code/a", kind: "anonymous" }),
    ]);

    expect(projects).toEqual([]);
    expect(standalone).toEqual([]);
  });

  it("treats a project with several conversations as ONE project", () => {
    // The user has one "place they work" even after ten conversations in it —
    // the old flat list showed ten look-alike rows instead.
    const { projects } = groupSessionsByProject([
      mk({ id: "s1", cwd: "/code/a", updated_at: 3 }),
      mk({ id: "s2", cwd: "/code/a", updated_at: 2 }),
      mk({ id: "s3", cwd: "/code/a", updated_at: 1 }),
    ]);

    expect(projects).toHaveLength(1);
    expect(projects[0].sessions).toHaveLength(3);
  });
});

describe("recentProjects", () => {
  it("caps the picker list at the most recently touched projects", () => {
    const sessions = Array.from({ length: 10 }, (_, i) =>
      mk({ id: `s${i}`, cwd: `/code/p${i}`, updated_at: i }),
    );

    const picked = recentProjects(sessions, 3);

    expect(picked.map((p) => p.name)).toEqual(["p9", "p8", "p7"]);
  });
});

describe("buildSessionRail", () => {
  it("keeps one recency-ordered list mixing folders and conversations", () => {
    const rail = buildSessionRail([
      mk({ id: "a1", cwd: "/code/a", updated_at: 400 }),
      mk({ id: "q1", cwd: "/scratch/q1", kind: "quick", updated_at: 300 }),
      mk({ id: "a2", cwd: "/code/a", updated_at: 200 }),
      mk({ id: "b1", cwd: "/code/b", updated_at: 100 }),
    ]);

    // /code/a has two conversations → a group, ordered by its newest child.
    expect(rail.map((e) => (e.kind === "project" ? e.project.name : e.session.id))).toEqual([
      "a",
      "q1",
      "b1",
    ]);
  });

  it("leaves a folder used once as a plain conversation row", () => {
    const rail = buildSessionRail([mk({ id: "b1", cwd: "/code/b" })]);

    expect(rail).toHaveLength(1);
    expect(rail[0].kind).toBe("session");
    expect(rail[0].kind === "session" && rail[0].projectName).toBe("b");
  });

  it("grows that folder into a group as soon as it holds a second one", () => {
    const rail = buildSessionRail([
      mk({ id: "b1", cwd: "/code/b", updated_at: 2 }),
      mk({ id: "b2", cwd: "/code/b", updated_at: 1 }),
    ]);

    expect(rail).toHaveLength(1);
    expect(rail[0].kind).toBe("project");
    expect(rail[0].kind === "project" && rail[0].project.sessions).toHaveLength(2);
  });

  it("marks a standalone conversation with no folder at all", () => {
    const rail = buildSessionRail([mk({ id: "q1", cwd: "/scratch/q1", kind: "quick" })]);

    expect(rail[0].kind === "session" && rail[0].projectName).toBeNull();
  });
});
