import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import fixture from "../../docs/acceptance/specialistbots-dm993-projection.json";
import { SpecialistEvidencePanel, parseSpecialistReport } from "./SpecialistEvidencePanel";

function upload(text: string, size?: number) {
  const file = new File([text], "report.json", { type: "application/json" });
  Object.defineProperty(file, "text", { value: async () => text });
  if (size !== undefined) Object.defineProperty(file, "size", { value: size });
  fireEvent.change(screen.getByLabelText("导入本地 QA/PM 报告"), { target: { files: [file] } });
}
describe("SpecialistBots evidence display boundary", () => {
  it("rejects unsupported schema, execution claims and contradictory states", () => {
    expect(parseSpecialistReport(JSON.stringify(fixture)).pm.finding_id).toBe("DM-993-001");
    for (const changes of [{ schema_version: 2 }, { autonomous_dispatch_enabled: true }, { qa_dot_autonomous: true }, { model_called: true }, { pm: { ...fixture.pm, state: "closed" } }, { qa: { ...fixture.qa, local_checks: "incomplete" } }]) {
      expect(() => parseSpecialistReport(JSON.stringify({ ...fixture, ...changes }))).toThrow();
    }
  });
  it("starts empty and displays a real report without claiming verification", async () => {
    render(<SpecialistEvidencePanel project="/project/a" />);
    expect(screen.getByText("自主执行关闭")).toBeInTheDocument();
    expect(screen.queryByText("DM-993-001")).not.toBeInTheDocument();
    upload(JSON.stringify(fixture));
    expect(await screen.findByText("DM-993-001")).toBeInTheDocument();
    expect(screen.getByText("报告声明材料一致")).toBeInTheDocument();
    expect(screen.getByText("待补执行证明")).toBeInTheDocument();
    expect(screen.getByText(/当前项目关联未验证/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /执行|修复|关闭任务/ })).not.toBeInTheDocument();
  });
  it("clears previous results on invalid or oversized import", async () => {
    render(<SpecialistEvidencePanel project="/project/a" />);
    upload(JSON.stringify(fixture)); await screen.findByText("DM-993-001");
    upload("{bad"); expect(await screen.findByRole("alert")).toHaveTextContent("无法显示报告");
    expect(screen.queryByText("DM-993-001")).not.toBeInTheDocument();
    upload("{}", 262145); await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("256 KiB"));
  });
  it("rejects late results from a different project and unmount", async () => {
    const { rerender, unmount } = render(<SpecialistEvidencePanel project="/project/a" />);
    let resolve!: (value: string) => void;
    const file = new File([], "slow.json");
    Object.defineProperty(file, "text", { value: () => new Promise<string>((done) => { resolve = done; }) });
    fireEvent.change(screen.getByLabelText("导入本地 QA/PM 报告"), { target: { files: [file] } });
    rerender(<SpecialistEvidencePanel project="/project/b" />);
    await act(async () => resolve(JSON.stringify(fixture)));
    expect(screen.queryByText("DM-993-001")).not.toBeInTheDocument();
    expect(screen.getByText("/project/b")).toBeInTheDocument();
    unmount();
  });
  it("renders hostile titles as text and keeps latest import only", async () => {
    render(<SpecialistEvidencePanel project="/project/a" />);
    let resolve!: (value: string) => void;
    const slow = new File([], "slow.json");
    Object.defineProperty(slow, "text", { value: () => new Promise<string>((done) => { resolve = done; }) });
    fireEvent.change(screen.getByLabelText("导入本地 QA/PM 报告"), { target: { files: [slow] } });
    const title = '<img src=x onerror="alert(1)">';
    upload(JSON.stringify({ ...fixture, pm: { ...fixture.pm, title } }));
    await screen.findByText(title);
    await act(async () => resolve(JSON.stringify(fixture)));
    expect(screen.getByText(title)).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });
  it("shows incomplete evidence conservatively and ignores picker cancellation", async () => {
    const missing = { ...fixture, qa: { local_checks: "incomplete", missing: ["green_log"], limitations: [] }, pm: { ...fixture.pm, state: "needs_evidence" } };
    render(<SpecialistEvidencePanel project="/project/a" />);
    upload(JSON.stringify(missing)); await screen.findByText("待补材料");
    fireEvent.change(screen.getByLabelText("导入本地 QA/PM 报告"), { target: { files: [] } });
    expect(screen.getByText("DM-993-001")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
  });
  it("treats inherited object keys as unknown limitation text", async () => {
    render(<SpecialistEvidencePanel project="/project/a" />);
    upload(JSON.stringify({ ...fixture, qa: { ...fixture.qa, limitations: [...fixture.qa.limitations, "__proto__", "constructor", "toString"] } }));
    expect(await screen.findByText("未验证项：__proto__")).toBeInTheDocument();
    expect(screen.getByText("未验证项：constructor")).toBeInTheDocument();
    expect(screen.getByText("未验证项：toString")).toBeInTheDocument();
  });

});
