# SpecialistBots 项目内只读证据视图

## 信息架构与主路径

使用现有Workspace项目头部与辅助面板，新增“QA/PM证据”入口；不混入会自动调度的任务活动面板。用户打开项目→打开QA/PM证据→选择本地S6投影JSON→查看QA报告声明、PM待办、原PR/head及待补证明→关闭返回原入口。默认空态，不内置真实报告冒充本次运行。

## 需求

- R1：复用Workspace辅助面板布局/键盘/焦点规范；无项目不显示入口。
- R2：仅通过用户文件选择读<=256KiB JSON；严格schema、类型、长度、状态及关闭标志验证，错误清除旧报告；不新增IPC读取/执行权限。
- R3：始终显示自主执行关闭、本地导入未认证、执行证明未验证；当前cwd和报告repo数字ID分别显示，关联未验证。consistent仅翻为“报告声明材料一致”，不是应用验收通过。
- R4：展示本地PM待办、发现ID、原PR/head、限制和缺项；needs_execution_provenance保持“待补执行证明”，没有派发/修复/关闭任务操作。
- R5：项目切换/卸载/连续导入时旧异步结果不能覆盖新状态；取消选文件不报错。窄屏可滚动，长ID/标题不溢出，文本不作为HTML执行。

## 验收与限制

Spec、Compatibility、Viewport、Payload、AI Collaboration。先失败unit tests、Workspace入口测试、真实Chromium生产组件用户路径及状态回归。UI演示用synthetic IPC，真实S6文件从浏览器文件上传进入；不称为Tauri桌面完整验收。无模型、测试执行、远端任务或持久PM更新；权限/隔离与执行证明未就绪，自主执行关闭。
