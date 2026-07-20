# CodeFactory 发布流水线性能：业务设计

## 问题

v1.48.0 的真实 Release workflow 用时约 24 分 22 秒。Windows 构建约 11 分 57 秒，随后 macOS 构建约 11 分 23 秒；两者没有产品依赖，却因为 draft release 的创建顺序被串行执行。与此同时，发布由不同 tag ref 触发，GitHub Actions cache 不能跨 tag 复用，Windows Rust sccache 为 0/521 命中，macOS 为 1/772 命中。

这会延长“确定性改进已合并”到“用户可下载安装”的等待时间，也让每次发布重复支付接近完整冷构建的成本。

## 目标

- Windows 与 macOS 在同一 draft release 准备完成后并行构建。
- Release workflow 从 `main` 显式启动并 checkout 指定 tag，使连续发布共享默认分支缓存作用域。
- 保留现有版本计算、安装包、updater、draft 单点发布和发布后 macOS 复验契约。
- 发布失败可重跑：已有 draft 可以复用，已发布 tag 不得被重复覆盖。

## 非目标

- 不改变 CodeFactory Agent、模型路由或 Terminal-Bench 行为。
- 不为单个 benchmark task 增加路径或判断。
- 不以删除安装包验证、签名或发布后复验换取速度。
- 不承诺每次构建的固定缓存命中率；依赖或源码大幅变化时允许重新编译。

## 产品价值

1. 功能合并后更快形成 Windows/macOS 可下载版本，缩短能力改进的上线闭环。
2. 连续发布可以复用未变化 Rust 对象和依赖缓存，减少托管 runner 时间。
3. 构建并行后，单个平台变慢不会把另一平台的构建时间完整叠加到关键路径。

## 成功标准

- Windows 与 macOS job 的运行时间区间发生重叠。
- runner 正常分配时，首轮结构性优化后的 Release 总时长不高于 15 分钟。
- 再下一次包含可复用 Rust 对象的发布中，两个平台的 Rust sccache 命中数均大于 0。
- 任一平台失败时 release 保持 draft，`finalize` 不得执行。
