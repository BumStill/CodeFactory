# SpecialistBots 项目内证据视图验收

规格：`docs/specs/feature-specs/specialistbots-evidence-view.md`。

| 需求 | 状态 | 证据 |
|---|---|---|
| R1 项目入口 | deferred | 原因：浏览器沙箱启动失败；后续：CF-SPECIALISTBOTS-UI-S7-BROWSER，补真实打开关闭焦点路径； Workspace真实组件jsdom入口/关闭回归；Chromium环境启动失败，真实键盘/焦点待验 |
| R2 文件边界 | done | src/components/SpecialistEvidencePanel.test.tsx::rejects unsupported schema； SpecialistEvidencePanel测试：版本、关闭标志、状态组合、256KiB、坏JSON清除旧报告 |
| R3 保守真实性 | done | src/components/SpecialistEvidencePanel.test.tsx::starts empty； 真实PR993投影导入回归；始终显示自主执行关闭、执行证明与项目关联未验证 |
| R4 本地待办 | done | src/components/SpecialistEvidencePanel.test.tsx::shows incomplete evidence； 展示DM-993-001、待补执行证明、QA报告声明、限制及原始标识；无执行/关闭按钮 |
| R5 异步与视口 | deferred | 原因：浏览器沙箱启动失败；后续：CF-SPECIALISTBOTS-UI-S7-BROWSER，补视口主题路径； 连续导入、跨项目旧读取、恶意标题及继承属性名回归；375/1280视口仍待浏览器实测 |

## 可复验路径

打开有cwd的项目工作区→头部“QA/PM 证据”→选择 `docs/acceptance/specialistbots-dm993-projection.json`→查看“待补执行证明”及缺项→展开原始标识。文件由S6真实PR993核对材料生成，是历史观察，不是当前远端查询，也不是岗位执行证明。视图只校验展示格式，未重新核对来源摘要或测试日志；关闭面板后导入状态不持久化。

## 验证证据

- `/workspace/s0-evidence/specialistbots-ui-red.log`：缺实现导入失败。
- `specialistbots-ui-qa-red.log`：独立QA发现继承属性渲染崩溃，新增测试1失败/6通过；修为仅查自有属性。
- `specialistbots-ui-regression.log`：面板与Workspace 70项通过；`specialistbots-ui-typecheck.log`：tsc通过；`specialistbots-ui-build.log`：Vite构建通过（既有chunk大小警告）。独立QA复验13项通过，未发现剩余代码阻断；整体UI验收待补。
- 浏览器脚本 `scripts/verify-specialistbots-headless.mjs`，入口 `specialistbots-acceptance.html` 使用生产Workspace组件、合成IPC/项目；真实文件通过浏览器上传。它不是Tauri桌面或真实自治E2E。
- 实际尝试日志 `specialistbots-browser.log`：Chromium启动即SIGABRT，SUID sandbox helper配置不正确。保持 `chromiumSandbox:true`，没有关闭沙箱、修改权限或绕过限制。没有截图或浏览器通过证据。
- 下一步在已有可用沙箱的桌面/CI上启动 `node node_modules/vite/bin/vite.js --host 127.0.0.1 --port 1468`，再运行 `node scripts/verify-specialistbots-headless.mjs`；如非Linux指定CHROMIUM_PATH。补查375/1280、亮暗主题、导入、伪造拒绝、Escape焦点返回与跨项目清空。浏览器脚本未成功执行到断言，不能声称其断言已验证。

## 交付边界

not live：仅本地分支，无推送、合并或部署。UI实地验收未完成，不能作为可发布通过项。独立QA和单元测试不替代该门禁。受认证项目/岗位、可信执行证明及资源隔离仍缺；不接真实派发或持久PM，不把文件投影叫自治。
