# M239：更新卡住时必须有出路

## 问题
Windows 用户没有任何任务，更新却一直卡住，界面只显示排队或正在启动，也没有 M238 的两个按钮。

## 根因
- M238 的按钮只在「有真实工作在跑」时显示，其他等待状态没有任何操作。
- 一次下载或安装失败后，失败记录占住全局唯一的「未决」位置，自动重试一次后永远挡住这个版本和之后所有版本。手动安装其他版本后同样挡住。
- 旧版本的更新任务没结束时，新版本入队直接报错。界面把报错显示成「1 项本地执行仍在运行」，这是假话。
- 后台失败不告诉界面，界面继续显示「正在启动下载」。

## 要求
- R1：已结束且没装上的尝试，不得挡住其他版本。它退出未决位置，记录保留在 `update_install_receipts_retired`。可能还在运行的尝试（`install_started`）继续挡住。
- R2：没有工作者持有的旧版本任务，让位给新版本。工作者正持有时不替换。
- R3：更新报错，或没有阻塞却 60 秒没开始下载，界面改为「更新没有开始」，不再声称有工作在运行，并提供「重试」和「手动下载」。选了立即更新后同样计时。收起的小标记自动展开。设置页同样处理。
- R4：「重试」由用户触发，清掉已结束的失败尝试后重新入队，可以越过自动重试只有一次的限制。

## 边界
- 已经卡住的 1.91.87 及更早版本拿不到这个修复，要手动安装一次。之后 R1 会自动清掉旧记录。
- 排队期间的具体失败原因没有传给界面，界面只给通用说明。

## 验收
- 单测：`a_failed_attempt_on_an_old_target_does_not_block_a_newer_target`、`an_attempt_still_in_flight_keeps_fencing_a_newer_target`、`user_retry_reopens_an_attempt_that_used_up_its_automatic_retry`、`a_newer_target_replaces_an_old_target_only_when_nothing_is_running_it`、`src/stores/updater.test.ts`。
- 真浏览器：`scripts/verify-update-stalled-headless.mjs`。
- 真实 Windows：从卡住的状态手动装上修复版后，下一次更新能走完下载、安装和重启。
