// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const installed = {
  id: "continuity-helper",
  name: "Continuity Helper",
  description: "Keeps the handoff visible",
  version: "1.0.0",
  author: "fixture",
  tags: [],
  enabled: false,
  path: "/synthetic/continuity-helper",
  source: "user" as const,
};

const mocks = vi.hoisted(() => ({
  skillList: { current: [] as (typeof installed)[] },
  catalogError: { current: null as string | null },
  loadSkills: vi.fn(),
  enableSkill: vi.fn(),
  disableSkill: vi.fn(),
  installFromUrl: vi.fn(),
  installMarketplace: vi.fn(),
  selectSourceDirectory: vi.fn(),
  importFromDirectory: vi.fn(),
  createSkill: vi.fn(),
  updateSkill: vi.fn(),
  deleteSkill: vi.fn(),
  getSkillDetail: vi.fn(),
  invoke: vi.fn(),
}));

vi.mock("../../stores/skills", () => ({
  useSkillsStore: () => ({
    ...mocks,
    skills: mocks.skillList.current,
    loading: false,
    catalogError: mocks.catalogError.current,
  }),
}));

vi.mock("../../stores/chat", () => ({
  useChatStore: () => null,
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));

import { SkillsPage, SkillsPanel } from "./SkillsPage";

beforeEach(() => {
  mocks.skillList.current = [];
  mocks.catalogError.current = null;
  for (const mock of Object.values(mocks)) {
    if (typeof mock === "function" && "mockReset" in mock) mock.mockReset();
  }
  mocks.loadSkills.mockResolvedValue(undefined);
  mocks.installFromUrl.mockResolvedValue(installed);
  mocks.installMarketplace.mockResolvedValue(installed);
  mocks.getSkillDetail.mockResolvedValue({
    manifest: installed,
    system_prompt: "Stay continuous.",
    slash_commands: [],
    has_tool_policy: false,
    tool_policy: null,
    review_fingerprint: "sha256:reviewed-continuity-helper",
  });
  mocks.enableSkill.mockResolvedValue(undefined);
});

describe("Skill lifecycle continuity", () => {
  it("moves directly from a successful install to explicit review and global enable", async () => {
    render(<SkillsPage onBack={() => {}} />);

    await userEvent.type(screen.getByPlaceholderText("从 URL 安装…"), "https://example.com/skill.json");
    await userEvent.click(screen.getByTitle("从 URL 安装技能"));

    expect(await screen.findByRole("heading", { name: "Continuity Helper" })).toBeInTheDocument();
    expect(screen.getByRole("status", { name: "安装结果" })).toHaveFocus();
    expect(screen.getByText("已安装，尚未启用。检查内容后再启用。")).toBeInTheDocument();
    expect(screen.getByText("Stay continuous.")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "检查并启用…" }));
    expect(screen.getByText(/当前版本将对所有项目生效/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "批准并在所有项目启用" })).toHaveFocus();

    await userEvent.click(screen.getByRole("button", { name: "批准并在所有项目启用" }));
    await waitFor(() => expect(mocks.enableSkill).toHaveBeenCalledWith(
      "continuity-helper",
      "sha256:reviewed-continuity-helper",
    ));
  });

  it("returns a chat-origin review only after the enable receipt succeeds", async () => {
    const onReviewEnabled = vi.fn();
    render(
      <SkillsPanel
        initialSkillId="continuity-helper"
        onReviewEnabled={onReviewEnabled}
      />,
    );

    await userEvent.click(await screen.findByRole("button", { name: "检查并启用…" }));
    expect(onReviewEnabled).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "批准并在所有项目启用" }));

    await waitFor(() => expect(onReviewEnabled).toHaveBeenCalledWith("continuity-helper"));
  });

  it("keeps the enable dialog visible while saving and after a failed enable", async () => {
    let rejectEnable: (reason: Error) => void = () => {};
    mocks.enableSkill.mockReturnValue(
      new Promise<void>((_resolve, reject) => {
        rejectEnable = reject;
      }),
    );
    render(<SkillsPage onBack={() => {}} />);

    await userEvent.type(screen.getByPlaceholderText("从 URL 安装…"), "https://example.com/skill.json");
    await userEvent.click(screen.getByTitle("从 URL 安装技能"));
    await userEvent.click(await screen.findByRole("button", { name: "检查并启用…" }));
    await userEvent.click(screen.getByRole("button", { name: "批准并在所有项目启用" }));

    const dialog = screen.getByRole("dialog");
    await waitFor(() => expect(dialog).toHaveFocus());
    await userEvent.tab();
    expect(dialog).toHaveFocus();
    await userEvent.keyboard("{Escape}");
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    await userEvent.click(dialog.parentElement!);
    expect(screen.getByRole("dialog")).toBeInTheDocument();

    rejectEnable(new Error("synthetic enable failure"));
    expect(await screen.findByRole("alert")).toHaveTextContent("synthetic enable failure");
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it("blocks approval and requires re-review when content changes after display", async () => {
    mocks.enableSkill.mockRejectedValueOnce(new Error(
      "SKILL_REVIEW_CONTENT_CHANGED: synthetic content drift",
    ));
    mocks.getSkillDetail
      .mockResolvedValueOnce({
        manifest: installed,
        system_prompt: "Displayed body.",
        slash_commands: [],
        has_tool_policy: false,
        tool_policy: null,
        review_fingerprint: "sha256:displayed",
      })
      .mockResolvedValueOnce({
        manifest: installed,
        system_prompt: "Changed body.",
        slash_commands: [],
        has_tool_policy: false,
        tool_policy: null,
        review_fingerprint: "sha256:changed",
      });
    render(<SkillsPage onBack={() => {}} />);

    await userEvent.type(screen.getByPlaceholderText("从 URL 安装…"), "https://example.com/skill.json");
    await userEvent.click(screen.getByTitle("从 URL 安装技能"));
    await userEvent.click(await screen.findByRole("button", { name: "检查并启用…" }));
    await userEvent.click(screen.getByRole("button", { name: "批准并在所有项目启用" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("SKILL_REVIEW_CONTENT_CHANGED");
    expect(screen.getByRole("button", { name: "批准并在所有项目启用" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(await screen.findByText("Changed body.")).toBeInTheDocument();
  });

  it("does not claim drift refresh succeeded when the latest detail failed to load", async () => {
    const displayedDetail = {
      manifest: installed,
      system_prompt: "Displayed body.",
      slash_commands: [],
      has_tool_policy: false,
      tool_policy: null,
      review_fingerprint: "sha256:displayed",
    };
    const changedDetail = {
      ...displayedDetail,
      system_prompt: "Changed after retry.",
      review_fingerprint: "sha256:changed",
    };
    mocks.enableSkill.mockRejectedValueOnce(new Error(
      "SKILL_REVIEW_CONTENT_CHANGED: synthetic content drift",
    ));
    mocks.getSkillDetail
      .mockResolvedValueOnce(displayedDetail)
      .mockRejectedValueOnce(new Error("synthetic latest-detail failure"))
      .mockResolvedValueOnce(changedDetail);
    render(<SkillsPage onBack={() => {}} />);

    await userEvent.type(screen.getByPlaceholderText("从 URL 安装…"), "https://example.com/skill.json");
    await userEvent.click(screen.getByTitle("从 URL 安装技能"));
    await userEvent.click(await screen.findByRole("button", { name: "检查并启用…" }));
    await userEvent.click(screen.getByRole("button", { name: "批准并在所有项目启用" }));

    expect(await screen.findByText(/最新内容加载失败/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "批准并在所有项目启用" })).toBeDisabled();
    expect(screen.queryByText(/已在后台刷新为最新内容/)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "重新加载最新详情" }));
    expect(await screen.findByText(/已在后台刷新为最新内容/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(await screen.findByText("Changed after retry.")).toBeInTheDocument();
  });

  it("keeps a successful enable visible when only the detail refresh fails", async () => {
    const displayedDetail = {
      manifest: installed,
      system_prompt: "Displayed body.",
      slash_commands: [],
      has_tool_policy: false,
      tool_policy: null,
      review_fingerprint: "sha256:displayed",
    };
    mocks.getSkillDetail
      .mockResolvedValueOnce(displayedDetail)
      .mockRejectedValueOnce(new Error("synthetic post-enable refresh failure"));
    render(<SkillsPage onBack={() => {}} />);

    await userEvent.type(screen.getByPlaceholderText("从 URL 安装…"), "https://example.com/skill.json");
    await userEvent.click(screen.getByTitle("从 URL 安装技能"));
    await userEvent.click(await screen.findByRole("button", { name: "检查并启用…" }));
    await userEvent.click(screen.getByRole("button", { name: "批准并在所有项目启用" }));

    expect(await screen.findByRole("button", { name: "禁用" })).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("已启用，但详情刷新失败");
    expect(screen.getByRole("button", { name: "重新加载详情" })).toBeInTheDocument();
  });

  it("reports disable failure without projecting a false disabled state", async () => {
    const enabled = { ...installed, enabled: true };
    mocks.skillList.current = [enabled];
    mocks.getSkillDetail.mockResolvedValue({
      manifest: enabled,
      system_prompt: "Enabled body.",
      slash_commands: [],
      has_tool_policy: false,
      tool_policy: null,
      review_fingerprint: "sha256:enabled",
    });
    mocks.disableSkill.mockRejectedValueOnce(new Error("synthetic disable failure"));
    render(<SkillsPage onBack={() => {}} />);

    await userEvent.click(screen.getByRole("button", { name: "查看 Continuity Helper" }));
    await userEvent.click(await screen.findByRole("button", { name: "禁用" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("禁用失败");
    expect(screen.getByRole("button", { name: "禁用" })).toBeInTheDocument();
  });

  it("ignores an older detail response after the user selects a different Skill", async () => {
    const second = { ...installed, id: "second-helper", name: "Second Helper" };
    mocks.skillList.current = [installed, second];
    let resolveFirst: (value: unknown) => void = () => {};
    let resolveSecond: (value: unknown) => void = () => {};
    mocks.getSkillDetail.mockImplementation((id: string) => new Promise((resolve) => {
      if (id === installed.id) resolveFirst = resolve;
      else resolveSecond = resolve;
    }));
    render(<SkillsPage onBack={() => {}} />);

    await userEvent.click(screen.getByRole("button", { name: "查看 Continuity Helper" }));
    await userEvent.click(screen.getByRole("button", { name: "查看 Second Helper" }));
    resolveSecond({
      manifest: second,
      system_prompt: "SECOND",
      slash_commands: [],
      has_tool_policy: false,
      tool_policy: null,
      review_fingerprint: "sha256:second",
    });
    expect(await screen.findByRole("heading", { name: "Second Helper" })).toBeInTheDocument();
    resolveFirst({
      manifest: installed,
      system_prompt: "FIRST LATE",
      slash_commands: [],
      has_tool_policy: false,
      tool_policy: null,
      review_fingerprint: "sha256:first",
    });
    await waitFor(() => expect(screen.queryByText("FIRST LATE")).not.toBeInTheDocument());
    expect(screen.getByRole("heading", { name: "Second Helper" })).toBeInTheDocument();
  });

  it("does not let a late post-disable refresh replace the newly selected Skill", async () => {
    const enabled = { ...installed, enabled: true };
    const second = { ...installed, id: "second-helper", name: "Second Helper" };
    const enabledDetail = {
      manifest: enabled,
      system_prompt: "ENABLED FIRST",
      slash_commands: [],
      has_tool_policy: false,
      tool_policy: null,
      review_fingerprint: "sha256:enabled-first",
    };
    const secondDetail = {
      manifest: second,
      system_prompt: "SECOND CURRENT",
      slash_commands: [],
      has_tool_policy: false,
      tool_policy: null,
      review_fingerprint: "sha256:second",
    };
    mocks.skillList.current = [enabled, second];
    let firstReads = 0;
    let resolveLateRefresh: (value: unknown) => void = () => {};
    mocks.getSkillDetail.mockImplementation((id: string) => {
      if (id === second.id) return Promise.resolve(secondDetail);
      firstReads += 1;
      if (firstReads === 1) return Promise.resolve(enabledDetail);
      return new Promise((resolve) => { resolveLateRefresh = resolve; });
    });
    render(<SkillsPage onBack={() => {}} />);

    await userEvent.click(screen.getByRole("button", { name: "查看 Continuity Helper" }));
    await userEvent.click(await screen.findByRole("button", { name: "禁用" }));
    await waitFor(() => expect(mocks.getSkillDetail).toHaveBeenCalledTimes(2));
    await userEvent.click(screen.getByRole("button", { name: "查看 Second Helper" }));
    expect(await screen.findByText("SECOND CURRENT")).toBeInTheDocument();

    resolveLateRefresh({
      ...enabledDetail,
      manifest: { ...enabled, enabled: false },
      system_prompt: "FIRST LATE AFTER DISABLE",
    });
    await waitFor(() => expect(screen.queryByText("FIRST LATE AFTER DISABLE")).not.toBeInTheDocument());
    expect(screen.getByRole("heading", { name: "Second Helper" })).toBeInTheDocument();
  });

  it("does not attach a late post-disable refresh failure to the newly selected Skill", async () => {
    const enabled = { ...installed, enabled: true };
    const second = { ...installed, id: "second-helper", name: "Second Helper" };
    mocks.skillList.current = [enabled, second];
    let firstReads = 0;
    let rejectLateRefresh: (reason: Error) => void = () => {};
    mocks.getSkillDetail.mockImplementation((id: string) => {
      if (id === second.id) {
        return Promise.resolve({
          manifest: second,
          system_prompt: "SECOND CURRENT",
          slash_commands: [],
          has_tool_policy: false,
          tool_policy: null,
          review_fingerprint: "sha256:second",
        });
      }
      firstReads += 1;
      if (firstReads === 1) {
        return Promise.resolve({
          manifest: enabled,
          system_prompt: "ENABLED FIRST",
          slash_commands: [],
          has_tool_policy: false,
          tool_policy: null,
          review_fingerprint: "sha256:enabled-first",
        });
      }
      return new Promise((_resolve, reject) => { rejectLateRefresh = reject; });
    });
    render(<SkillsPage onBack={() => {}} />);

    await userEvent.click(screen.getByRole("button", { name: "查看 Continuity Helper" }));
    await userEvent.click(await screen.findByRole("button", { name: "禁用" }));
    await waitFor(() => expect(mocks.getSkillDetail).toHaveBeenCalledTimes(2));
    await userEvent.click(screen.getByRole("button", { name: "查看 Second Helper" }));
    expect(await screen.findByText("SECOND CURRENT")).toBeInTheDocument();

    rejectLateRefresh(new Error("FIRST LATE REFRESH FAILURE"));
    await waitFor(() => expect(screen.queryByText(/FIRST LATE REFRESH FAILURE/)).not.toBeInTheDocument());
    expect(screen.getByRole("heading", { name: "Second Helper" })).toBeInTheDocument();
  });

  it("does not project A after its disable mutation returns behind a selection of B", async () => {
    const enabled = { ...installed, enabled: true };
    const second = { ...installed, id: "second-helper", name: "Second Helper" };
    mocks.skillList.current = [enabled, second];
    let resolveDisable: () => void = () => {};
    mocks.disableSkill.mockReturnValue(new Promise<void>((resolve) => { resolveDisable = resolve; }));
    mocks.getSkillDetail.mockImplementation(async (id: string) => ({
      manifest: id === second.id ? second : enabled,
      system_prompt: id === second.id ? "SECOND CURRENT" : "ENABLED FIRST",
      slash_commands: [],
      has_tool_policy: false,
      tool_policy: null,
      review_fingerprint: `sha256:${id}`,
    }));
    render(<SkillsPage onBack={() => {}} />);

    await userEvent.click(screen.getByRole("button", { name: "查看 Continuity Helper" }));
    await userEvent.click(await screen.findByRole("button", { name: "禁用" }));
    await waitFor(() => expect(mocks.disableSkill).toHaveBeenCalledWith(installed.id));
    await userEvent.click(screen.getByRole("button", { name: "查看 Second Helper" }));
    expect(await screen.findByText("SECOND CURRENT")).toBeInTheDocument();

    resolveDisable();
    await waitFor(() => expect(screen.getByRole("heading", { name: "Second Helper" })).toBeInTheDocument());
    expect(screen.queryByText("ENABLED FIRST")).not.toBeInTheDocument();
  });

  it("does not attach A's late disable mutation failure to B", async () => {
    const enabled = { ...installed, enabled: true };
    const second = { ...installed, id: "second-helper", name: "Second Helper" };
    mocks.skillList.current = [enabled, second];
    let rejectDisable: (reason: Error) => void = () => {};
    mocks.disableSkill.mockReturnValue(new Promise<void>((_resolve, reject) => { rejectDisable = reject; }));
    mocks.getSkillDetail.mockImplementation(async (id: string) => ({
      manifest: id === second.id ? second : enabled,
      system_prompt: id === second.id ? "SECOND CURRENT" : "ENABLED FIRST",
      slash_commands: [],
      has_tool_policy: false,
      tool_policy: null,
      review_fingerprint: `sha256:${id}`,
    }));
    render(<SkillsPage onBack={() => {}} />);

    await userEvent.click(screen.getByRole("button", { name: "查看 Continuity Helper" }));
    await userEvent.click(await screen.findByRole("button", { name: "禁用" }));
    await waitFor(() => expect(mocks.disableSkill).toHaveBeenCalledWith(installed.id));
    await userEvent.click(screen.getByRole("button", { name: "查看 Second Helper" }));
    expect(await screen.findByText("SECOND CURRENT")).toBeInTheDocument();

    rejectDisable(new Error("FIRST LATE DISABLE FAILURE"));
    await waitFor(() => expect(screen.queryByText(/FIRST LATE DISABLE FAILURE/)).not.toBeInTheDocument());
    expect(screen.getByRole("heading", { name: "Second Helper" })).toBeInTheDocument();
  });

  it("keeps the current edit form owned by a pending successful save", async () => {
    mocks.skillList.current = [installed];
    let resolveUpdate: () => void = () => {};
    mocks.updateSkill.mockReturnValue(new Promise<void>((resolve) => { resolveUpdate = resolve; }));
    render(<SkillsPage onBack={() => {}} />);

    await userEvent.click(screen.getByRole("button", { name: "查看 Continuity Helper" }));
    await userEvent.click(await screen.findByRole("button", { name: "编辑" }));
    await userEvent.click(screen.getByRole("button", { name: "保存为未启用版本" }));
    await waitFor(() => expect(mocks.updateSkill).toHaveBeenCalled());
    const dialog = screen.getByRole("dialog", { name: "编辑技能" });
    expect(screen.getByRole("button", { name: "取消" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "关闭" })).toBeDisabled();
    expect(screen.getByDisplayValue("Continuity Helper")).toBeDisabled();
    await waitFor(() => expect(dialog).toHaveFocus());
    await userEvent.tab();
    expect(dialog).toHaveFocus();
    await userEvent.click(dialog.parentElement!);
    expect(screen.getByRole("dialog", { name: "编辑技能" })).toBeInTheDocument();

    resolveUpdate();
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "编辑技能" })).not.toBeInTheDocument());
  });

  it("keeps the edit form and restores cancellation after a pending save fails", async () => {
    mocks.skillList.current = [installed];
    let rejectUpdate: (reason: Error) => void = () => {};
    mocks.updateSkill.mockReturnValue(new Promise<void>((_resolve, reject) => { rejectUpdate = reject; }));
    render(<SkillsPage onBack={() => {}} />);

    await userEvent.click(screen.getByRole("button", { name: "查看 Continuity Helper" }));
    await userEvent.click(await screen.findByRole("button", { name: "编辑" }));
    await userEvent.click(screen.getByRole("button", { name: "保存为未启用版本" }));
    await waitFor(() => expect(mocks.updateSkill).toHaveBeenCalled());
    expect(screen.getByRole("button", { name: "取消" })).toBeDisabled();

    rejectUpdate(new Error("synthetic update failure"));
    expect(await screen.findByText(/synthetic update failure/)).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "编辑技能" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "取消" })).toBeEnabled();
    expect(screen.getByDisplayValue("Continuity Helper")).toBeEnabled();
  });

  it("treats an edit receipt as success when only the follow-up detail refresh fails", async () => {
    mocks.skillList.current = [installed];
    mocks.getSkillDetail
      .mockResolvedValueOnce({
        manifest: installed,
        system_prompt: "FIRST EDITABLE",
        slash_commands: [],
        has_tool_policy: false,
        tool_policy: null,
        review_fingerprint: "sha256:first",
      })
      .mockRejectedValueOnce(new Error("synthetic post-update refresh failure"));
    render(<SkillsPage onBack={() => {}} />);

    await userEvent.click(screen.getByRole("button", { name: "查看 Continuity Helper" }));
    await userEvent.click(await screen.findByRole("button", { name: "编辑" }));
    await userEvent.clear(screen.getByDisplayValue("Continuity Helper"));
    await userEvent.type(screen.getByPlaceholderText("例如 周报助手"), "Updated Helper");
    await userEvent.click(screen.getByRole("button", { name: "保存为未启用版本" }));

    await waitFor(() => expect(screen.queryByRole("dialog", { name: "编辑技能" })).not.toBeInTheDocument());
    expect(await screen.findByRole("alert")).toHaveTextContent("已保存为未启用版本，但详情刷新失败");
    expect(screen.getByRole("heading", { name: "Updated Helper" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "重新加载详情" })).toBeInTheDocument();
  });

  it("keeps a cancelled CAS reload cancelled when its response arrives late", async () => {
    const displayedDetail = {
      manifest: installed,
      system_prompt: "Displayed body.",
      slash_commands: [],
      has_tool_policy: false,
      tool_policy: null,
      review_fingerprint: "sha256:displayed",
    };
    let resolveManualReload: (value: unknown) => void = () => {};
    mocks.enableSkill.mockRejectedValueOnce(new Error(
      "SKILL_REVIEW_CONTENT_CHANGED: synthetic content drift",
    ));
    mocks.getSkillDetail
      .mockResolvedValueOnce(displayedDetail)
      .mockRejectedValueOnce(new Error("synthetic automatic reload failure"))
      .mockReturnValueOnce(new Promise((resolve) => { resolveManualReload = resolve; }));
    render(<SkillsPage onBack={() => {}} />);

    await userEvent.type(screen.getByPlaceholderText("从 URL 安装…"), "https://example.com/skill.json");
    await userEvent.click(screen.getByTitle("从 URL 安装技能"));
    await userEvent.click(await screen.findByRole("button", { name: "检查并启用…" }));
    await userEvent.click(screen.getByRole("button", { name: "批准并在所有项目启用" }));
    await userEvent.click(await screen.findByRole("button", { name: "重新加载最新详情" }));
    await userEvent.click(screen.getByRole("button", { name: "取消" }));

    resolveManualReload({
      ...displayedDetail,
      system_prompt: "LATE CHANGED BODY",
      review_fingerprint: "sha256:late",
    });
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.queryByText("LATE CHANGED BODY")).not.toBeInTheDocument();
  });

  it("closes an idle enable dialog with Escape and restores focus to the review action", async () => {
    render(<SkillsPage onBack={() => {}} />);

    await userEvent.type(screen.getByPlaceholderText("从 URL 安装…"), "https://example.com/skill.json");
    await userEvent.click(screen.getByTitle("从 URL 安装技能"));
    const reviewButton = await screen.findByRole("button", { name: "检查并启用…" });
    await userEvent.click(reviewButton);
    await userEvent.keyboard("{Escape}");

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(reviewButton).toHaveFocus();
  });

  it("keeps the exact installed id and offers retry when detail loading fails", async () => {
    mocks.getSkillDetail.mockRejectedValueOnce(new Error("synthetic detail failure"));
    render(<SkillsPage onBack={() => {}} />);

    await userEvent.type(screen.getByPlaceholderText("从 URL 安装…"), "https://example.com/skill.json");
    await userEvent.click(screen.getByTitle("从 URL 安装技能"));

    expect(await screen.findByRole("alert")).toHaveTextContent("continuity-helper");
    mocks.getSkillDetail.mockResolvedValue({
      manifest: installed,
      system_prompt: "Stay continuous.",
      slash_commands: [],
      has_tool_policy: false,
      tool_policy: null,
      review_fingerprint: "sha256:reviewed-continuity-helper",
    });
    await userEvent.click(screen.getByRole("button", { name: "重试加载" }));
    expect(await screen.findByRole("heading", { name: "Continuity Helper" })).toBeInTheDocument();
  });

  it("shows every batch import outcome and lets the user inspect every successful skill", async () => {
    const failed = {
      path: "/synthetic/broken-skill",
      error: "manifest.json 解析失败",
    };
    const second = { ...installed, id: "second-helper", name: "Second Helper" };
    mocks.selectSourceDirectory.mockResolvedValue({
      source_handle: "skill-source-synthetic",
      display_path: "/synthetic/batch",
    });
    mocks.importFromDirectory.mockResolvedValue({ succeeded: [installed, second], failed: [failed] });
    mocks.getSkillDetail.mockImplementation(async (id: string) => ({
      manifest: id === second.id ? second : installed,
      system_prompt: id === second.id ? "Second body." : "Stay continuous.",
      slash_commands: [],
      has_tool_policy: false,
      tool_policy: null,
      review_fingerprint: `sha256:reviewed-${id}`,
    }));
    render(<SkillsPage onBack={() => {}} />);

    await userEvent.click(screen.getByTitle(/从本地目录导入/));

    expect(mocks.importFromDirectory).toHaveBeenCalledWith("skill-source-synthetic");
    expect(await screen.findByRole("heading", { name: "Continuity Helper" })).toBeInTheDocument();
    const summary = screen.getByRole("status", { name: "批量导入结果" });
    expect(summary).toHaveTextContent("成功 2 个，失败 1 个");
    expect(summary).toHaveTextContent("continuity-helper");
    await userEvent.click(screen.getByRole("button", { name: /检查已安装的 second-helper/ }));
    expect(await screen.findByRole("heading", { name: "Second Helper" })).toBeInTheDocument();
    expect(summary).toHaveTextContent("/synthetic/broken-skill");
  });

  it("preserves earlier OpenClaw successes when a later source fails", async () => {
    mocks.invoke.mockResolvedValue([
      { name: "First", description: "ok", path: "/synthetic/first", source_handle: "skill-source-first", already_installed: false },
      { name: "Second", description: "fails", path: "/synthetic/second", source_handle: "skill-source-second", already_installed: false },
    ]);
    mocks.importFromDirectory
      .mockResolvedValueOnce({ succeeded: [installed], failed: [] })
      .mockRejectedValueOnce(new Error("synthetic second-source failure"));
    render(<SkillsPage onBack={() => {}} />);

    await userEvent.click(screen.getByRole("button", { name: "一键导入 OpenClaw 技能" }));

    expect(await screen.findByRole("heading", { name: "Continuity Helper" })).toBeInTheDocument();
    const summary = screen.getByRole("status", { name: "批量导入结果" });
    expect(summary).toHaveTextContent("成功 1 个，失败 1 个");
    expect(summary).toHaveTextContent("/synthetic/second");
    expect(summary).toHaveTextContent("synthetic second-source failure");
  });

  it("keeps a marketplace install reviewable when the catalog refresh fails", async () => {
    mocks.catalogError.current = "目录刷新失败：refresh failed";
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "fetch_marketplace_skills") {
        return [{
          ...installed,
          system_prompt: "Stay continuous.",
          slash_commands: [],
          installed: false,
        }];
      }
      throw new Error(`unexpected command ${command}`);
    });
    render(<SkillsPage onBack={() => {}} />);

    await userEvent.click(screen.getByRole("button", { name: "市场" }));
    await userEvent.click(await screen.findByTitle("安装"));

    expect(mocks.installMarketplace).toHaveBeenCalledWith("continuity-helper");
    expect(await screen.findByRole("heading", { name: "Continuity Helper" })).toBeInTheDocument();
    expect(screen.getByText(/目录刷新失败：refresh failed/)).toBeInTheDocument();
  });

  it("shows a catalog load failure instead of claiming that no skills are installed", () => {
    mocks.catalogError.current = "技能目录加载失败：synthetic catalog failure";
    render(<SkillsPage onBack={() => {}} />);

    expect(screen.getByRole("alert")).toHaveTextContent("synthetic catalog failure");
    expect(screen.queryByText("未安装任何技能")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "重试加载技能目录" })).toBeInTheDocument();
  });

  it("shows a corrupt installed directory without offering activation", async () => {
    const corrupt = {
      ...installed,
      id: "broken-skill",
      name: "broken-skill",
      description: "SKILL_MANIFEST_INVALID",
      lifecycle_status: "corrupt" as const,
    };
    mocks.skillList.current = [corrupt];
    mocks.getSkillDetail.mockResolvedValue({
      manifest: corrupt,
      system_prompt: "",
      slash_commands: [],
      has_tool_policy: false,
      tool_policy: null,
      review_fingerprint: null,
    });
    render(<SkillsPage onBack={() => {}} />);

    await userEvent.click(screen.getByRole("button", { name: "查看 broken-skill" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("不会进入任何任务上下文");
    expect(screen.queryByRole("button", { name: "检查并启用…" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "移除此损坏项…" }));
    expect(screen.getByRole("dialog", { name: "永久移除“broken-skill”？" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(mocks.deleteSkill).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "移除此损坏项…" }));
    await userEvent.click(screen.getByRole("button", { name: "永久移除已安装副本" }));
    expect(mocks.deleteSkill).toHaveBeenCalledWith("broken-skill");
  });
});
