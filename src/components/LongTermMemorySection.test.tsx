// SPDX-License-Identifier: Apache-2.0
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { LongTermMemorySection } from "./LongTermMemorySection";

const api = vi.hoisted(() => ({
  listLongTermMemories: vi.fn(),
  editLongTermMemory: vi.fn(),
  deleteLongTermMemory: vi.fn(),
  setLongTermMemoryEnabled: vi.fn(),
}));
vi.mock("../lib/longTermMemory", () => api);

const memory = {
  id: "m1",
  content: "回复用中文",
  kind: "preference",
  scope: "user",
  repo_label: "",
  source_session_id: "s1",
  source_kind: "user_message",
  status: "active",
  created_at: 1,
  source_text: "记住：回复用中文",
};

describe("LongTermMemorySection", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.listLongTermMemories.mockResolvedValue({ enabled: true, memories: [memory] });
    api.deleteLongTermMemory.mockResolvedValue(1);
    api.editLongTermMemory.mockResolvedValue(undefined);
    api.setLongTermMemoryEnabled.mockResolvedValue(undefined);
  });

  it("shows where a memory came from", async () => {
    render(<LongTermMemorySection />);
    await userEvent.click(await screen.findByRole("button", { name: "查看来源" }));
    expect(screen.getByText("记住：回复用中文")).toBeTruthy();
  });

  it("deletes only after a second confirmation", async () => {
    render(<LongTermMemorySection />);
    await userEvent.click(await screen.findByRole("button", { name: "删除" }));
    expect(api.deleteLongTermMemory).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "确认删除" }));
    await waitFor(() => expect(api.deleteLongTermMemory).toHaveBeenCalledWith("m1"));
  });

  it("shows why an edit was refused and keeps the draft", async () => {
    api.editLongTermMemory.mockRejectedValue("内容里有密钥或凭据，不能存入长期记忆");
    render(<LongTermMemorySection />);
    await userEvent.click(await screen.findByRole("button", { name: "修改" }));
    await userEvent.click(screen.getByRole("button", { name: "保存" }));
    expect((await screen.findByRole("alert")).textContent).toContain("密钥");
    expect(screen.getByLabelText("修改记忆内容")).toBeTruthy();
  });

  it("turns all memory off from one switch", async () => {
    render(<LongTermMemorySection />);
    await userEvent.click(await screen.findByRole("checkbox", { name: "使用长期记忆" }));
    expect(api.setLongTermMemoryEnabled).toHaveBeenCalledWith(false);
  });
});
