# README 更新机制：架构设计

## 组件

| 组件 | 责任 |
|---|---|
| `README.md` | 稳定产品契约；包含 evergreen maintenance marker 和版本中立下载入口 |
| `.github/pull_request_template.md` | 给作者展示唯一的机器字段和判断提示 |
| `tools/governance/validate_readme_contract.py` | 静态 README 检查、PR body 决策解析、required diff 检查 |
| `.github/workflows/ci.yml` | 在已有 required `check` job 中运行 validator，避免新增规则集依赖 |
| `.github/workflows/readme-review.yml` | 每月/手动执行静态检查并幂等创建 review issue，不改正文 |
| `docs/principles/readme-update-cadence.md` | 跨 agent 的规范来源 |

## 数据流

```text
PR body + event payload
          │
          ▼
  README contract validator ── static: marker/headings/links/version
          │                    └─ PR: decision/reason + required README diff
          ▼
       required check → merge → existing release notes pipeline

monthly schedule → static validator → one review issue → human PR (if needed)
```

## 合同

- `README-Update` 和 `README-Update-Reason` 必须各出现一次；理由不能是占位符。
- 静态层拒绝精确 `vX.Y.Z`/`X.Y.Z`，要求 `releases/latest`，并解析 README 内相对链接。
- PR event 使用 `pull_request.base.sha...HEAD` 判断 `README.md` 是否真实变更；缺 base
  SHA 时 required 决策失败关闭。
- 非 PR push 只跑静态层，避免把 merge commit 的 GitHub 自动文案误当作作者决策。
- monthly workflow 只允许创建 issue；不执行 git 写入、提交或推送。

## 可靠性与取舍

- 复用现有 `check` required job，避免修改 GitHub ruleset；Linux agent-bridge 的
  unittest 还会覆盖 validator 的边界逻辑。
- 不让 release workflow 回写 README，避免每个版本产生噪音和 merge 竞争。
- 机器无法推断“用户可见”语义，所以把意图显式交给作者并要求理由；静态检查负责
  可机械证明的部分，月度 issue 负责语义复核。
- issue 创建按月份查询 open issue 后再创建，重复调度不会产生重复提醒；权限只给
  `contents: read` 和 `issues: write`。
