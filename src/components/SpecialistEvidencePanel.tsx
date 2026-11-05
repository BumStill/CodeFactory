// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState } from "react";

const MAX_BYTES = 256 * 1024;
type Data = Record<string, unknown>;
function object(value: unknown): Data {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("格式不受支持");
  return value as Data;
}
function check(value: unknown): asserts value { if (!value) throw new Error("格式或状态不受支持"); }
function text(value: unknown, max = 512): string { check(typeof value === "string" && value.length > 0 && value.length <= max); return value; }
function sha(value: unknown, length = 64): string { const s = text(value, length); check(new RegExp(`^[a-f0-9]{${length}}$`).test(s)); return s; }
function strings(value: unknown): string[] { check(Array.isArray(value) && value.length <= 64); return value.map((s) => text(s)); }
function positive(value: unknown): number { check(Number.isSafeInteger(value) && (value as number) > 0); return value as number; }

export interface SpecialistReport {
  observation: string;
  source: { repository: number; pr: number; head: string; evidence: string };
  qa: { consistent: boolean; limitations: string[]; missing: string[]; target?: string; passed?: number };
  pm: { task_id: string; finding_id: string; title: string; state: "needs_evidence" | "needs_execution_provenance"; branch?: string; base?: string };
}
/** Display schema only. Imported JSON is not authenticated or independently reverified. */
export function parseSpecialistReport(raw: string): SpecialistReport {
  check(new TextEncoder().encode(raw).length <= MAX_BYTES);
  const data = object(JSON.parse(raw));
  check(data.schema_version === 1 && data.evidence_trust === "local_supplied" && data.original_user_incident_resolved === "unverified");
  for (const flag of ["qa_dot_autonomous", "pm_dot_autonomous", "autonomous_dispatch_enabled", "model_called", "tests_executed_by_this_tool", "remote_writes", "persistent_pm_update"]) check(data[flag] === false);
  const source = object(data.source), qa = object(data.qa), pm = object(data.pm);
  const head = sha(source.head, 40);
  check(pm.delivery_state === "not_verified");
  const consistent = qa.local_checks === "consistent";
  check(consistent || qa.local_checks === "incomplete");
  check(pm.state === (consistent ? "needs_execution_provenance" : "needs_evidence"));
  const limitations = strings(qa.limitations), missing = strings(qa.missing);
  if (consistent) {
    check(missing.length === 0);
    for (const key of ["test_execution_not_attested", "red_source_digest_not_recorded", "synthetic_inputs_not_original_incident"]) check(limitations.includes(key));
    check(pm.original_review_head === head);
    sha(pm.fix_source_sha256);
  } else check(missing.length > 0);
  const taskId = text(pm.task_id, 72); check(/^finding-[a-f0-9]{64}$/.test(taskId));
  return {
    observation: sha(data.observation_id),
    source: { repository: positive(source.repository_id), pr: positive(source.pr), head, evidence: sha(source.evidence_sha256) },
    qa: { consistent, limitations, missing, target: consistent ? text(qa.target_test) : undefined, passed: consistent ? positive(qa.green_passed) : undefined },
    pm: { task_id: taskId, finding_id: text(pm.finding_id, 64), title: text(pm.title, 500), state: pm.state as SpecialistReport["pm"]["state"], branch: consistent ? text(pm.fix_branch, 200) : undefined, base: consistent ? sha(pm.fix_base, 40) : undefined },
  };
}
const labels: Record<string, string> = {
  test_execution_not_attested: "缺少可信执行器签发的测试证明",
  red_source_digest_not_recorded: "红阶段源码摘要未在执行时记录",
  synthetic_inputs_not_original_incident: "合成输入结果不能证明原用户故障已解决",
  green_log: "绿色测试日志", red_log: "失败测试日志", verification: "修复回执",
  fix_source: "修复源码", red_run: "红阶段运行记录", green_run: "绿阶段运行记录",
  complete_source_evidence: "完整来源证据",
};

export function SpecialistEvidencePanel({ project }: { project: string }) {
  const [report, setReport] = useState<SpecialistReport | null>(null);
  const [filename, setFilename] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const generation = useRef(0);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    generation.current++;
    setReport(null); setFilename(""); setError(""); setLoading(false);
    if (input.current) input.current.value = "";
    return () => { generation.current++; };
  }, [project]);
  const importFile = async (file?: File) => {
    if (!file) return;
    const current = ++generation.current;
    setReport(null); setFilename(""); setError(""); setLoading(true);
    try {
      if (file.size > MAX_BYTES) throw new Error("文件超过 256 KiB");
      const parsed = parseSpecialistReport(await file.text());
      if (current !== generation.current) return;
      setReport(parsed); setFilename(file.name);
    } catch {
      if (current === generation.current) setError(file.size > MAX_BYTES ? "无法显示报告：文件超过 256 KiB。" : "无法显示报告：JSON 格式、版本或关闭状态不受支持。请选择有效的本地交接报告。");
    } finally {
      if (current === generation.current) setLoading(false);
    }
  };
  return <section aria-label="QA/PM 证据内容" className="flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto p-4 text-label text-gray-300">
    <div className="mb-4 rounded-lg border border-status-warning/30 bg-status-warning-soft p-3">
      <p className="font-semibold text-status-warning">自主执行关闭</p>
      <p className="mt-1">本地导入，执行证明未验证。此视图不会运行 QA/PM 岗位或关闭任务。</p>
    </div>
    <p className="text-caption text-gray-500">当前项目</p><p className="mb-3 break-all">{project}</p>
    <label className="mb-3 block text-label">导入本地 QA/PM 报告
      <input ref={input} type="file" accept=".json,application/json" className="mt-2 block min-h-11 w-full min-w-0 text-caption file:mr-2 file:rounded file:border file:border-border file:bg-surface-2 file:px-3 file:py-2 file:text-gray-200" onChange={(event) => { void importFile(event.target.files?.[0]); event.target.value = ""; }} />
    </label>
    {loading && <p role="status">正在读取本地报告…</p>}
    {error && <p role="alert" className="mb-3 rounded border border-status-error/30 p-3 text-status-error">{error}</p>}
    {!report && !loading && <p className="text-gray-500">尚未载入报告。选择证据核对工具生成的 JSON；不会自动扫描项目或读取其它文件。</p>}
    {report && <div className="space-y-4 break-words [overflow-wrap:anywhere]">
      <p className="text-caption text-gray-500">已导入：{filename}</p>
      <p className="rounded bg-surface-2 p-3">当前项目关联未验证。报告声明仓库 ID：{report.source.repository}，PR #{report.source.pr}；不是当前远端状态查询。</p>
      <div><h3 className="font-semibold">本地 PM 待办</h3><p className="mt-1 text-status-warning">{report.pm.state === "needs_execution_provenance" ? "待补执行证明" : "待补材料"}</p><p className="mt-2">{report.pm.title}</p><p className="text-caption text-gray-500">发现编号</p><p>{report.pm.finding_id}</p></div>
      <div><h3 className="font-semibold">QA 证据观察</h3><p className="mt-1">{report.qa.consistent ? "报告声明材料一致" : "报告材料不完整"}</p><p className="text-caption text-gray-500">这里只校验显示格式，未在应用内重新执行或验证测试。</p>
        {report.qa.target && <><p className="mt-2">报告记录 {report.qa.passed} 项测试通过（未独立核验）</p><p className="text-caption">{report.qa.target}</p></>}
      </div>
      <div><h3 className="font-semibold">待补与限制</h3><ul className="mt-1 list-disc space-y-1 pl-5">{[...report.qa.limitations, ...report.qa.missing].map((key, i) => <li key={`${i}:${key}`}>{Object.prototype.hasOwnProperty.call(labels, key) ? labels[key] : `未验证项：${key}`}</li>)}</ul></div>
      <details className="rounded border border-border p-3"><summary className="cursor-pointer">查看原始标识与版本</summary><dl className="mt-3 space-y-2 text-caption">
        {[['原审查 head', report.source.head], ['修复基线', report.pm.base], ['修复分支', report.pm.branch], ['本地任务键', report.pm.task_id], ['观察摘要', report.observation], ['来源证据摘要', report.source.evidence]].map(([label, value]) => value && <div key={label}><dt className="text-gray-500">{label}</dt><dd className="break-all">{value}</dd></div>)}
      </dl></details>
      <p className="text-caption text-gray-500">本地待办仅供查看，未持久更新 PM 系统。权限和隔离条件未完成前，不提供自主派发。</p>
    </div>}
  </section>;
}
