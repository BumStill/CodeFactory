// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import type { Session } from "../../lib/tauri";
import type { DraftSession } from "../../stores/chat";
const state = vi.hoisted(() => ({ sessions: [] as Session[], activeSession: null as Session | null,
  draftSession: null as DraftSession | null, sendMessage: vi.fn(), createSession: vi.fn() }));
vi.mock("../../stores/chat", () => ({ useChatStore: (selector: (s: typeof state) => unknown) => selector(state) }));
import { ProjectPmEntry } from "./ProjectPmEntry";
const project = (id: string, cwd: string, kind: Session["kind"] = "project"): Session => ({ id, cwd, kind, title: "fixture", model_id: "none", created_at: 1, updated_at: 1, total_input_tokens: 0, total_output_tokens: 0 });
const draft = (id: string, cwd: string | null, anonymous = false): DraftSession => ({ id, cwd, anonymous, modelId: "none", text: "" });
const open = () => fireEvent.click(screen.getByRole("button", { name: "打开当前项目 PM Bot" }));
beforeEach(() => { state.sessions = []; state.activeSession = null; state.draftSession = null; vi.clearAllMocks(); });
describe("Workspace-owned ProjectPmEntry", () => {
  it("shows only the current project and runtime unavailable, without creating work", () => {
    state.sessions = [project("a", "/project/A")];
    render(<ProjectPmEntry sessionId="a" />); open();
    expect(screen.getByRole("region", { name: "当前项目 PM Bot" })).toHaveTextContent("/project/A");
    expect(screen.getByRole("status")).toHaveTextContent("运行时尚未接入");
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(state.sendMessage).not.toHaveBeenCalled(); expect(state.createSession).not.toHaveBeenCalled();
  });
  it("ignores a stale active session and unrelated draft when selecting the current session", () => {
    state.sessions = [project("a", "/project/A"), project("b", "/project/B")];
    state.activeSession = state.sessions[0]; state.draftSession = draft("other-draft", "/private-draft");
    render(<ProjectPmEntry sessionId="b" />); open();
    expect(screen.getByText("/project/B")).toBeInTheDocument();
    expect(screen.queryByText("/project/A")).not.toBeInTheDocument(); expect(screen.queryByText("/private-draft")).not.toBeInTheDocument();
  });
  it("uses only the matching nonanonymous project draft", () => {
    state.sessions = [project("a", "/project/A")]; state.activeSession = state.sessions[0];
    state.draftSession = draft("draft-b", "/project/B");
    render(<ProjectPmEntry sessionId="draft-b" />); open();
    expect(screen.getByText("/project/B")).toBeInTheDocument(); expect(screen.queryByText("/project/A")).not.toBeInTheDocument();
  });
  it.each(["anonymous", "quick"] as const)("hides %s sessions even with a cwd", (kind) => {
    state.sessions = [project("private", "/secret", kind)]; state.activeSession = project("stale", "/other");
    render(<ProjectPmEntry sessionId="private" />);
    expect(screen.queryByRole("button", { name: "打开当前项目 PM Bot" })).not.toBeInTheDocument(); expect(screen.queryByText("/secret")).not.toBeInTheDocument();
  });
  it("hides anonymous draft, empty scope and unknown id instead of falling back", () => {
    state.activeSession = project("stale", "/private"); state.draftSession = draft("draft", "/secret", true);
    const view = render(<ProjectPmEntry sessionId="draft" />);
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    state.draftSession = draft("draft", null); view.rerender(<ProjectPmEntry sessionId="draft" />);
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    view.rerender(<ProjectPmEntry sessionId="missing" />); expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
  it("does not let a stale project list expose an active anonymous session with the same id", () => {
    state.sessions = [project("a", "/secret")]; state.activeSession = project("a", "/secret", "anonymous");
    render(<ProjectPmEntry sessionId="a" />); expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
  it("hides conflicting snapshots and unsupported kinds instead of choosing a project", () => {
    state.sessions = [project("a", "/project/A")]; state.activeSession = project("a", "/project/B");
    const view = render(<ProjectPmEntry sessionId="a" />);
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    state.activeSession = null; state.sessions = [project("a", "/project/A", "future-kind" as Session["kind"])];
    view.rerender(<ProjectPmEntry sessionId="a" />); expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
  it("preserves legacy project kind and accepts an exactly matching newly opened session", () => {
    state.activeSession = { ...project("a", "/project/A"), kind: undefined };
    render(<ProjectPmEntry sessionId="a" />); open(); expect(screen.getByText("/project/A")).toBeInTheDocument();
  });
  it("clears an expanded panel when scope becomes anonymous", () => {
    state.sessions = [project("a", "/project/A")];
    const view = render(<ProjectPmEntry sessionId="a" />); open();
    state.draftSession = draft("private", "/private", true); view.rerender(<ProjectPmEntry sessionId="private" />);
    expect(screen.queryByRole("region")).not.toBeInTheDocument(); expect(screen.queryByText("/project/A")).not.toBeInTheDocument();
    view.rerender(<ProjectPmEntry sessionId="a" />); expect(screen.queryByRole("region")).not.toBeInTheDocument();
  });
  it("resets expanded scope on project changes and returns focus on Escape", () => {
    state.sessions = [project("a", "/project/A"), project("b", "/project/B")];
    const view = render(<ProjectPmEntry sessionId="a" />); open();
    expect(screen.getByRole("heading", { name: "当前项目 PM Bot" })).toHaveFocus();
    fireEvent.keyDown(screen.getByRole("region"), { key: "Escape" });
    expect(screen.getByRole("button", { name: "打开当前项目 PM Bot" })).toHaveFocus(); expect(screen.queryByRole("region")).not.toBeInTheDocument();
    open(); view.rerender(<ProjectPmEntry sessionId="b" />);
    expect(screen.queryByRole("region")).not.toBeInTheDocument(); open(); expect(screen.getByText("/project/B")).toBeInTheDocument();
    view.rerender(<ProjectPmEntry sessionId="a" />); expect(screen.queryByRole("region")).not.toBeInTheDocument();
  });
});
