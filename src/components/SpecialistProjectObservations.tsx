// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState } from "react";
import { invoke, type GitRemoteConfig } from "../lib/tauri";
interface Binding { id: string; repo: string; repo_id: number; actor_id: number; pr: number; head: string; role: string }
interface Observation { id: string; observed_at: string; pm_state: string; runs: {id:number;attempt:number;status:string;conclusion:string|null}[] }
interface RecordView { binding: Binding; active: boolean; observations: Observation[] }
interface ProjectStatus {epoch:number;records:RecordView[]}
function status(value: ProjectStatus): ProjectStatus {
  if (!value || !Number.isSafeInteger(value.epoch) || value.epoch < 0 || !Array.isArray(value.records)) throw new Error("unsupported project status");
  return value;
}
/** Authenticated read-only source observations; no model, test, or remote write dispatch. */
export function SpecialistProjectObservations({ project }: {project:string}) {
  const [remotes,setRemotes]=useState<GitRemoteConfig[]>([]);
  const [remote,setRemote]=useState(""); const [repo,setRepo]=useState(""); const [pr,setPr]=useState("");
  const [records,setRecords]=useState<RecordView[]>([]); const [busy,setBusy]=useState(false); const [error,setError]=useState("");
  const generation=useRef(0); const [epoch,setEpoch]=useState<number|null>(null);
  useEffect(()=>{
    const version=++generation.current;
    setRecords([]);setEpoch(null);setRemotes([]);setRemote("");setRepo("");setPr("");setError("");setBusy(false);
    void Promise.all([invoke<GitRemoteConfig[]>("list_git_remotes"),invoke<ProjectStatus>("list_specialist_observations",{project})]).then(([r,rows])=>{
      if(version!==generation.current)return;
      setRemotes(r.filter(x=>x.provider==="github" && x.base_url.replace(/\/$/,"")==="https://api.github.com"));setRecords(status(rows).records);setEpoch(rows.epoch);
    }).catch(()=>{if(version===generation.current)setError("无法读取项目观察或连接配置，请检查应用后端。");});
    return ()=>{generation.current++;};
  },[project]);
  async function perform(command:string,args:Record<string,unknown>={}) {
    const version=++generation.current;setBusy(true);setError("");
    try {
      await invoke(command,{project,...args});
      const [r,rows]=await Promise.all([invoke<GitRemoteConfig[]>("list_git_remotes"),invoke<ProjectStatus>("list_specialist_observations",{project})]);
      if(version===generation.current) {
        setRecords(status(rows).records);setEpoch(rows.epoch);
        setRemotes(r.filter(x=>x.provider==="github" && x.base_url.replace(/\/$/,"")==="https://api.github.com"));
      }
    } catch {if(version===generation.current)setError("查询或授权未完成：请检查连接、权限和当前 head；Stop 后需重新显式绑定。已有观察仅供历史查看。");}
    finally{if(version===generation.current)setBusy(false);}
  }
  const button="min-h-11 rounded border border-border bg-surface-2 px-3 py-2 text-caption disabled:opacity-50";
  const field="mt-1 block min-h-11 w-full min-w-0 rounded border border-border bg-surface-0 px-2 text-gray-200";
  return <div className="mb-5 space-y-3 border-b border-border pb-4 [overflow-wrap:anywhere]">
    <h3 className="font-semibold">项目 QA/PM 只读观察</h3>
    <p className="text-caption text-gray-500">使用已有 GitHub 连接认证账户。目录仅用于本地分区；以下绑定由你明确指定，不从目录推断远端身份。自主执行保持关闭，不运行审查、测试或修复。应用重启或凭据变化后需重新绑定；历史观察仍可查看。</p>
    <label className="block">GitHub 连接<select className={field} value={remote} onChange={e=>setRemote(e.target.value)}><option value="">选择连接</option>{remotes.map(r=><option key={r.id} value={r.id}>{r.name}</option>)}</select></label>
    {remotes.length===0 && <p className="text-caption">请先在现有远程 Git 设置中配置 api.github.com 连接。</p>}
    <label className="block">仓库 owner/repo<input className={field} maxLength={200} value={repo} onChange={e=>setRepo(e.target.value)}/></label>
    <label className="block">PR 编号<input className={field} inputMode="numeric" value={pr} maxLength={10} onChange={e=>setPr(e.target.value)}/></label>
    <p className="text-caption">授权范围：当前项目中的 QA 只读取证与 PM 本地观察。绑定当前 open PR 的精确 head；head 变化后需重新绑定。</p>
    <div className="flex flex-wrap gap-2"><button className={button} disabled={busy||epoch===null||!remote||!repo||!/^\d+$/.test(pr)||Number(pr)<1} onClick={()=>void perform("bind_specialist_project",{remoteId:remote,repo,pr:Number(pr),expectedEpoch:epoch})}>绑定并授权只读观察</button>
    <button className={button} onClick={()=>void perform("stop_specialist_observations")}>停止并撤销本项目查询授权</button></div>
    {busy&&<p role="status">正在处理只读观察…</p>}{error&&<p role="alert" className="text-status-error">{error}</p>}
    {records.map(({binding:b,active,observations})=><article key={b.id} className="space-y-2 rounded border border-border p-3">
      <p>{b.repo} · PR #{b.pr}</p><p className="text-caption">已绑定账户 ID {b.actor_id} · 仓库 ID {b.repo_id}</p><p className="break-all text-caption">head：{b.head}</p>
      <p>{active?"仅授权只读取证":"历史绑定：需重新授权"}</p>
      <button className={button} disabled={busy||!active} onClick={()=>void perform("query_specialist_observation",{bindingId:b.id})}>查询此 head</button>
      {observations.length===0?<p className="text-caption">尚无持久观察。</p>:<p>本地 PM 待办：<span className="text-status-warning">待补执行证明</span><span className="block break-all text-caption">任务键：{b.id}</span></p>}
      {observations.map(o=><div key={o.id} className="space-y-1 border-t border-border pt-2 text-caption">
        <p>观察时间：{o.observed_at}</p>
        {o.runs.length===0?<p>该 head 暂无 CI 证据</p>:o.runs.map(r=><p key={`${r.id}:${r.attempt}`}>GitHub run {r.id} / attempt {r.attempt}：{r.status} / {r.conclusion??"尚无结论"}</p>)}
        <p>仅记录 GitHub 返回的运行事实；不是发现已修复或可信执行证明。此记录已保存到当前项目。</p>
      </div>)}
    </article>)}
  </div>;
}
