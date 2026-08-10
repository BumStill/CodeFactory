# 方案：缩短 CI 时长

> 状态: **待批准**（提案）
>
> 日期: 2026-08-05
>
> 实测样本: run 31354756400（main, success）

---

## 现状

| Job | 耗时 |
| --- | --- |
| `check` (windows-latest) | **583s ≈ 9.7 分钟** |
| `agent-bridge-linux` | 42s |

`check` 是唯一瓶颈，且**串行**。步骤分布：

| 耗时 | 步骤 |
| --- | --- |
| 179s | Cargo test（根包） |
| 104s | Vitest |
| 92s | Evolution executable closed-loop smoke |
| 62s | **Cargo check** |
| 32s | Rust cache |
| 17s | Type-check frontend |
| 15s | Cargo test (agent-headless) |
| 11s | Setup Node |
| 11s | Evolution workbench headless viewport gate |
| 9s | Cargo test (agent-loop) |
| ≤7s | 其余 6 步 |

---

## 改进项（按收益/风险排序）

### 1. 删掉 `cargo check` —— 省 **62s**，零覆盖损失

```yaml
run: cargo check --manifest-path src-tauri/Cargo.toml   # 154 行
run: cargo test  --manifest-path src-tauri/Cargo.toml   # 162 行
```

两者**作用域完全相同**（同一 manifest，都没有 `-p` 限定）。`cargo test` 必须先完成编译才能跑，`check` 的类型检查结果被它完全覆盖。这是同一份代码编译两遍。

唯一的理论差异：`check` 用 dev profile、`test` 用 test profile，因此 `#[cfg(test)]` 之外的编译错误理论上可能只被前者发现——但 `test` profile 同样编译全部非测试代码，所以这个差异在实践中不存在。

**风险：低。** 建议直接删。

### 2. 三条 `cargo test` 合并为一条 `--workspace` —— 省约 **15–20s**

```yaml
cargo test --manifest-path src-tauri/Cargo.toml --quiet            # 179s
cargo test ... -p codefactory-agent-loop --quiet                   #   9s
cargo test ... -p codefactory-agent-headless --quiet               #  15s
```

三次独立调用各自重做依赖解析与链接。合成 `--workspace` 后编译产物共享。

**风险：中。** 现有注释解释了为什么分开写（根包 `cargo test` 不覆盖 workspace 成员）。`--workspace` 覆盖面**更大**而非更小，但输出会混在一起，某个 crate 失败时定位稍麻烦。**建议保留分步但去掉重复编译**——或者接受输出合并。需要判断。

### 3. 前后端并行 —— 省约 **130s**（最大单项）

当前所有步骤在一个 job 里串行。前端链（Setup Node 11s + Type-check 17s + Vitest 104s ≈ 132s）与 Rust 链（Rust cache 32s + Cargo test 179s + 两个 crate 24s ≈ 235s）**互不依赖**。

拆成两个并行 job 后，`check` 的墙钟时间由 `max(前端链, Rust 链)` 决定而非求和。

**代价**：`check` 目前是**一个** required status check；拆分后需要在 ruleset 里把必需检查改成两个，否则会出现"其中一个没被要求"的漏洞。属于门禁配置变更，需要用户批准。

**风险：中。** 收益最大，但触及分支保护配置。

### 4. Evolution closed-loop smoke 92s —— 需要先取证

第三大单项。它是真实可执行闭环，未必能压缩，但值得看清 92s 花在哪（构建？启动？等待？）。**未调查，不列入建议**。

---

## 汇总

| 项 | 收益 | 风险 | 是否需用户批准 |
| --- | --- | --- | --- |
| 1. 删 `cargo check` | 62s | 低 | 否 |
| 2. 合并 cargo test | 15–20s | 中 | 否 |
| 3. 前后端并行 | ~130s | 中 | **是**（改必需检查） |
| 4. Evolution smoke | ? | ? | 先取证 |

只做第 1 项：**583s → 约 521s**。
第 1+3 项：**583s → 约 390s（-33%）**。

---

## 建议

先做第 1 项——零风险、立刻见效、不需要任何配置变更。

第 3 项收益最大但要改 ruleset 的必需检查列表；考虑到本轮已经因为门禁配置踩过几次坑（strict、merge queue 不可用、凭空造出的签名前置），这一项应当单独做并明确确认。
