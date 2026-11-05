// SPDX-License-Identifier: Apache-2.0
import { beforeEach, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
vi.mock("../lib/tauri", () => ({invoke:vi.fn()}));
import { invoke } from "../lib/tauri";
import { SpecialistProjectObservations } from "./SpecialistProjectObservations";
const b={id:"binding",project:"/project/a",repo:"owner/repo",repo_id:7,actor_id:9,pr:1,head:"a".repeat(40),epoch:0,remote_id:"remote",role:"qa_read_pm_observe"};
let records: unknown[]; let epoch=0;
beforeEach(()=>{records=[];epoch=0;vi.mocked(invoke).mockReset();vi.mocked(invoke).mockImplementation(async(cmd)=>{
 if(cmd==="list_git_remotes") return [{id:"remote",name:"GitHub",provider:"github",base_url:"https://api.github.com",default_repo:"owner/repo"}];
 if(cmd==="list_specialist_observations") return {epoch,records};
 if(cmd==="bind_specialist_project") {records=[{binding:b,active:true,observations:[]}];return b;}
 if(cmd==="query_specialist_observation") {records=[{binding:b,active:true,observations:[{id:"o",observed_at:"2026-11-05T00:00:00Z",pm_state:"needs_execution_provenance",runs:[]}]}];return;}
 if(cmd==="stop_specialist_observations") {epoch++;records=[{binding:b,active:false,observations:[]}];return;}
 return null;
});});
it("binds explicitly, queries exact binding and reloads persisted observations",async()=>{
 const view=render(<SpecialistProjectObservations project="/project/a"/>);
 await screen.findByRole("option",{name:"GitHub"});
 fireEvent.change(screen.getByLabelText("GitHub 连接"),{target:{value:"remote"}});
 fireEvent.change(screen.getByLabelText("仓库 owner/repo"),{target:{value:"owner/repo"}});
 fireEvent.change(screen.getByLabelText("PR 编号"),{target:{value:"1"}});
 fireEvent.click(screen.getByRole("button",{name:"绑定并授权只读观察"}));
 expect(invoke).toHaveBeenCalledWith("bind_specialist_project",{project:"/project/a",remoteId:"remote",repo:"owner/repo",pr:1,expectedEpoch:0});
 fireEvent.click(await screen.findByRole("button",{name:"查询此 head"}));
 await screen.findByText("该 head 暂无 CI 证据");
 expect(invoke).toHaveBeenCalledWith("query_specialist_observation",{project:"/project/a",bindingId:"binding"});
 view.unmount();render(<SpecialistProjectObservations project="/project/a"/>);
 expect(await screen.findByText("待补执行证明")).toBeInTheDocument();
 expect(screen.queryByText("审查通过")).toBeNull();
});
it("ignores stale list responses after switching projects",async()=>{
 let resolve!:(value:unknown)=>void;
 vi.mocked(invoke).mockImplementation(async(cmd,args)=>cmd==="list_git_remotes"?[]:(args as {project:string}).project==="/project/a"?new Promise(done=>{resolve=done;}):{epoch:0,records:[]});
 const view=render(<SpecialistProjectObservations project="/project/a"/>);
 view.rerender(<SpecialistProjectObservations project="/project/b"/>);
 await act(async()=>resolve({epoch:0,records:[{binding:b,active:true,observations:[]}]}));
 expect(screen.queryByText("owner/repo")).toBeNull();
});
it("Stop remains available during a query and late replies do not reactivate authorization",async()=>{
 records=[{binding:b,active:true,observations:[]}];
 let finish!:()=>void;
 const normal=vi.mocked(invoke).getMockImplementation()!;
 vi.mocked(invoke).mockImplementation((cmd,args)=>cmd==="query_specialist_observation"?new Promise<void>(done=>{finish=done;}) as never:normal(cmd,args));
 render(<SpecialistProjectObservations project="/project/a"/>);
 fireEvent.click(await screen.findByRole("button",{name:"查询此 head"}));
 const stop=screen.getByRole("button",{name:"停止并撤销本项目查询授权"});expect(stop).not.toBeDisabled();fireEvent.click(stop);
 await screen.findByText("历史绑定：需重新授权");
 await act(async()=>finish());
 expect(screen.getByRole("button",{name:"查询此 head"})).toBeDisabled();
 expect(invoke).toHaveBeenCalledWith("stop_specialist_observations",{project:"/project/a"});
});

it("uses the newly returned epoch only for a fresh explicit bind after Stop",async()=>{
 render(<SpecialistProjectObservations project="/project/a"/>);
 await screen.findByRole("option",{name:"GitHub"});
 fireEvent.click(screen.getByRole("button",{name:"停止并撤销本项目查询授权"}));
 await screen.findByText("历史绑定：需重新授权");
 fireEvent.change(screen.getByLabelText("GitHub 连接"),{target:{value:"remote"}});
 fireEvent.change(screen.getByLabelText("仓库 owner/repo"),{target:{value:"owner/repo"}});
 fireEvent.change(screen.getByLabelText("PR 编号"),{target:{value:"1"}});
 fireEvent.click(screen.getByRole("button",{name:"绑定并授权只读观察"}));
 expect(invoke).toHaveBeenCalledWith("bind_specialist_project",{project:"/project/a",remoteId:"remote",repo:"owner/repo",pr:1,expectedEpoch:1});
 await screen.findByText("仅授权只读取证");
});
