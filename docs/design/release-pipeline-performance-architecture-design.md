# CodeFactory 发布流水线性能：架构设计

## 当前结构

```text
Auto Release -> push main/tag -> Release(tag ref)
                                -> changelog -> Windows -> macOS -> finalize -> published smoke
```

问题有两个：`needs: build-windows` 强制 macOS 串行；每个 tag 是独立 cache scope，导致上一版本写入的 sccache 无法被下一版本读取。

## 目标结构

```text
Auto Release(main)
  -> commit + tag + push
  -> workflow_dispatch Release(ref=main, tag=vX.Y.Z)

Release(main cache scope, checkout tag)
  -> changelog(tag) -----------+
  -> prepare draft(tag, SHA) --+-> Windows(tag) --+
                               +-> macOS(tag) ----+-> finalize -> published macOS smoke
```

## 关键决策

### 显式输入 tag

`release.yml` 只接受 `workflow_dispatch.inputs.tag`。每个读取源码的 job 都显式 checkout 该 tag，构建 SHA 由 `prepare-release` 解析并输出，避免把 workflow 所在的 `main` SHA 错写进安装包。

### draft 先于双平台构建

`prepare-release` 生成 release notes 后创建 draft。若同一 tag 的 draft 已存在则复用；若 release 已发布则失败，防止重跑覆盖公开产物。Windows 和 macOS 只上传互不重名的资产，不再承担 draft 初始化。

### 缓存与源码身份分离

workflow run 固定在 `main`，因此 GitHub Actions cache 可以跨连续发布恢复；源码、版本、changelog、构建 SHA 和产物仍全部来自输入 tag。缓存只影响编译速度，不改变产物身份。

### 单点发布保持不变

`finalize` 同时依赖 Windows 与 macOS，组装跨平台 `latest.json` 后才把 draft 改为公开。任一构建或安装版 smoke 失败都会阻止发布。

## 失败与恢复

| Failure | Result | Recovery |
| --- | --- | --- |
| tag 不存在或不指向 commit | `prepare-release` 失败 | 修正 tag 后重新 dispatch |
| draft 创建后单平台失败 | 保留 draft，不公开 | 修复后对同一 tag 重跑 |
| release 已公开后重复 dispatch | 明确失败，不覆盖资产 | 创建新版本或使用独立历史复验 workflow |
| Auto Release push 成功但 dispatch 失败 | Auto Release 标红，tag 保留 | 手动对 `main` dispatch 同一 tag |

## Compatibility

- tag 命名、版本文件、Conventional Commit 槽位不变。
- Windows NSIS、macOS DMG、updater tarball、签名和 `latest.json` 契约不变。
- 公开 release 仍只有一个 finalize 点，发布后 DMG 仍从匿名公开 URL 复验。
