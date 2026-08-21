// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());

vi.mock("../lib/tauri", () => ({ invoke }));

import { useSkillsStore, type SkillManifest } from "./skills";

const installed: SkillManifest = {
  id: "continuity-helper",
  name: "Continuity Helper",
  description: "Keeps the mutation receipt visible",
  version: "1.0.0",
  author: "fixture",
  tags: [],
  enabled: false,
  path: "/synthetic/continuity-helper",
  source: "user",
};

beforeEach(() => {
  invoke.mockReset();
  useSkillsStore.setState({ skills: [], loading: false, catalogError: null });
});

describe("Skill mutation continuity", () => {
  it("binds enable to the exact fingerprint returned by the detail view", async () => {
    invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "enable_skill") {
        expect(args).toEqual({
          id: "continuity-helper",
          expectedReviewFingerprint: "sha256:displayed",
        });
        return undefined;
      }
      if (command === "list_skills") return [{ ...installed, enabled: true }];
      throw new Error(`unexpected command ${command}`);
    });
    useSkillsStore.setState({ skills: [installed], loading: false, catalogError: null });

    await useSkillsStore
      .getState()
      .enableSkill("continuity-helper", "sha256:displayed");

    expect(useSkillsStore.getState().skills[0]?.enabled).toBe(true);
  });

  it("returns and projects a successful URL install even when catalog refresh fails", async () => {
    invoke.mockImplementation(async (command: string) => {
      if (command === "install_skill_from_url") return installed;
      if (command === "list_skills") throw new Error("synthetic refresh failure");
      throw new Error(`unexpected command ${command}`);
    });

    const result = await useSkillsStore.getState().installFromUrl("https://example.com/skill.json");

    expect(result).toEqual(installed);
    expect(useSkillsStore.getState().skills).toEqual([installed]);
    expect(useSkillsStore.getState().catalogError).toContain("synthetic refresh failure");
  });

  it("keeps every successful batch item when catalog refresh fails", async () => {
    invoke.mockImplementation(async (command: string) => {
      if (command === "install_skill_from_directory") {
        return {
          succeeded: [installed],
          failed: [{ path: "/synthetic/broken", error: "invalid manifest" }],
        };
      }
      if (command === "list_skills") throw new Error("synthetic refresh failure");
      throw new Error(`unexpected command ${command}`);
    });

    const result = await useSkillsStore.getState().importFromDirectory("skill-source-synthetic");

    expect(invoke).toHaveBeenCalledWith("install_skill_from_directory", {
      sourceHandle: "skill-source-synthetic",
    });
    expect(result.succeeded).toEqual([installed]);
    expect(result.failed).toHaveLength(1);
    expect(useSkillsStore.getState().skills).toEqual([installed]);
    expect(useSkillsStore.getState().catalogError).toContain("synthetic refresh failure");
  });

  it("projects a marketplace receipt before a failed catalog refresh", async () => {
    invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "install_marketplace_skill") {
        expect(args).toEqual({ skillId: "continuity-helper" });
        return installed;
      }
      if (command === "list_skills") throw new Error("synthetic refresh failure");
      throw new Error(`unexpected command ${command}`);
    });

    const result = await useSkillsStore.getState().installMarketplace("continuity-helper");

    expect(result).toEqual(installed);
    expect(useSkillsStore.getState().skills).toEqual([installed]);
    expect(useSkillsStore.getState().catalogError).toContain("synthetic refresh failure");
  });

  it("records an initial catalog failure without replacing known skills", async () => {
    useSkillsStore.setState({ skills: [installed], loading: false, catalogError: null });
    invoke.mockRejectedValue(new Error("synthetic initial failure"));

    await expect(useSkillsStore.getState().loadSkills()).rejects.toThrow("synthetic initial failure");

    expect(useSkillsStore.getState().skills).toEqual([installed]);
    expect(useSkillsStore.getState().catalogError).toContain("synthetic initial failure");
  });

  it("does not let an older catalog response erase a newer install receipt", async () => {
    let resolveOlderList: (skills: SkillManifest[]) => void = () => {};
    let listCalls = 0;
    invoke.mockImplementation((command: string) => {
      if (command === "install_skill_from_url") return Promise.resolve(installed);
      if (command === "list_skills") {
        listCalls += 1;
        if (listCalls === 1) {
          return new Promise<SkillManifest[]>((resolve) => { resolveOlderList = resolve; });
        }
        return Promise.resolve([installed]);
      }
      throw new Error(`unexpected command ${command}`);
    });

    const olderLoad = useSkillsStore.getState().loadSkills();
    await useSkillsStore.getState().installFromUrl("https://example.com/skill.json");
    resolveOlderList([]);
    await olderLoad;

    expect(useSkillsStore.getState().skills).toEqual([installed]);
    expect(useSkillsStore.getState().loading).toBe(false);
  });

  it("does not let an older catalog response resurrect a deleted Skill", async () => {
    useSkillsStore.setState({ skills: [installed], loading: false, catalogError: null });
    let resolveOlderList: (skills: SkillManifest[]) => void = () => {};
    let listCalls = 0;
    invoke.mockImplementation((command: string) => {
      if (command === "delete_skill") return Promise.resolve(undefined);
      if (command === "list_skills") {
        listCalls += 1;
        if (listCalls === 1) {
          return new Promise<SkillManifest[]>((resolve) => { resolveOlderList = resolve; });
        }
        return Promise.resolve([]);
      }
      throw new Error(`unexpected command ${command}`);
    });

    const olderLoad = useSkillsStore.getState().loadSkills();
    await useSkillsStore.getState().deleteSkill(installed.id);
    resolveOlderList([installed]);
    await olderLoad;

    expect(useSkillsStore.getState().skills).toEqual([]);
    expect(useSkillsStore.getState().loading).toBe(false);
  });
});
