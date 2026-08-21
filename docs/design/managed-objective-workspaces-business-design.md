# Objective 受管工作区：业务设计

## 问题

CodeFactory 当前把会话选择的 `cwd` 同时当作项目入口、主 agent 写入目录和 delivery 目标。旧分支在 PR 合并后仍留在根 checkout 时，下一次本地会话可继续往该分支提交，导致重复历史、冲突 PR 和恢复逻辑改写用户根目录。

## 用户承诺

- 选择项目不等于授权 CodeFactory 接管当前 branch。
- 一次代码修改 Objective 只拥有一个受管工作区和一个新分支。
- 用户根 checkout 的 branch、dirty 文件和 reflog 是受保护资产。
- “PR 已开”“CI 通过”“已合并”“工作区已清理”是四个不同状态。

## 产品边界

首期覆盖主聊天的 `Implement/Deliver`、restart reattach 和 delivery identity。非 Git 目录保持现有本地执行；subagent 继承主工作区与自动 closeout 进入后续兼容阶段，但本期不得继续把 subagent diff merge 回用户根目录。

## 成功指标

- 新代码 Objective 从用户根 checkout 直接产生 commit 的次数为 0。
- terminal PR 分支被另一 Objective 复用的次数为 0。
- workspace identity 冲突后的 Git 副作用为 0。
- 合并后 24 小时内安全 closeout 成功率可观测，保留必须有结构化原因。
