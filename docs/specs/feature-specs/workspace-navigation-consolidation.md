# Workspace 顶栏与 Welcome 信息架构收敛

## Requirements Traceability

| Req ID | 要求 | 验证 |
| --- | --- | --- |
| CF-NAV-R1 | Workspace 顶栏只保留会话侧栏收展、模型、推理强度、Git、检查点和设置；新建会话只从侧栏进入 | component + real app |
| CF-NAV-R2 | 主题、画像、进化、评测、资源、AI Coding OS 不再作为 Workspace 顶栏按钮 | negative component + screenshot |
| CF-NAV-R3 | 设置「功能」用带说明的卡片到达五个现有能力页 | route component + real app |
| CF-NAV-R4 | 独立规范/计划入口删除，仓库文档与会话内部执行为唯一入口 | repository-intent tests + real app |
| CF-NAV-R5 | Welcome 使用独立 1×28 趋势摘要，Settings 继续使用 7 行日历 | component + keyboard + browser geometry |
| CF-NAV-R6 | Welcome Hero、用量卡与建议任务在 1366×768、800×600、375×812 无整页横向溢出 | headless screenshots + real app |
| CF-NAV-R7 | 低频入口移动不得破坏模型、Git、检查点、用量详情与原能力页 | focused regression + full suite |
| CF-NAV-R8 | PR+CI、真实 App 与精确发布产物验证前保持 `not live` | evidence pack |
| CF-NAV-R9 | 会话侧栏可随时收起，收起后顶栏保留明确的恢复控制；不得同时显示两个新建入口 | interaction component + real app |
| CF-NAV-R10 | 主会话区不得常驻任务树或执行流；运行、待处理、失败仅用不超过 30px 的任务活动入口提示，详细控制进入按需抽屉 | interaction component + real app |
| CF-NAV-R11 | 左侧会话栏采用单一紧凑新建入口、时间分组和扁平高密度会话行；768px 高窗口自然显示至少 10 条，单行不超过 46px | component geometry + real app |
| CF-NAV-R12 | 成功工具调用使用不超过 30px 的低强调活动行，连续三项以上归组；运行、权限、失败保持高可见且失败首行默认外露 | timeline component + real app |

## Primary User Path

用户在 Workspace 顶栏处理当前会话并收展左侧会话栏；新建会话只使用侧栏右上角的紧凑「＋」，不在顶栏重复出现。会话正文始终是视觉主角；后台任务通过紧凑活动入口进入抽屉，成功命令折叠为低强调活动行；点击唯一的设置入口，在「功能」中打开画像、进化审查、能力评测、资源中心或 AI Coding OS。新会话 Welcome 先看到紧凑今日用量与 28 天趋势，再选择一个建议任务。规范来自当前代码库，计划由会话内部执行，不出现独立工作台。

## Applicable Harnesses

- Spec Harness：CF-NAV-R1..R12。
- Viewport Harness：顶栏与 Welcome 三视口几何、无溢出、首屏可达。
- Compatibility Harness：现有 App views、Settings tabs、task provenance 与 Settings 年度地图。
- Observation/Release Harness：真实 Tauri、PR+CI、安装包与 exact artifact。
- AI Collaboration Harness：设计审查结论、假设、失败测试与真实路径证据。

## 完成边界

单元测试或截图单独通过不算完成。必须同时证明入口归属、路由可达、规范/计划删除、Welcome/Settings 两种趋势拓扑、三视口和真实发布产物。
