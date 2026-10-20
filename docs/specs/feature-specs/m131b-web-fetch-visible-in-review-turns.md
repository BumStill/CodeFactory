# M131b：只读会话看得见 web_fetch，ref 列举不再算「变更动作」

## 现场证据

- 会话 `357f833f-d6d1-4a66-9589-6a31c642b7b0`（v1.91.0，thin / 只读意图）：
  用户说「只读：读一下 https://en.wikipedia.org/wiki/Unix_philosophy」。
- 模型整轮 **没有调用过 `web_fetch`**（v1.91.0 已上线该工具），而是：
  1. `read_file docs/specs/architecture-constraints.md`（成功，但不解决问题）；
  2. `bash` 读仓库现状（`git status` + `gh pr view`），其中含
     `git branch --show-current` 的同一条命令 **被只读门禁拒绝**，提示
     「已跳过与当前显式只读意图冲突的 `bash` 变更动作」；
  3. `browser_session open` 拿到 `browser_pairing_required`；
  4. 最后以「需要先在设置 → 浏览器会话里准备并加载扩展」收尾，页面从未被读取。
- 直接后果：用户给出一个公开 URL，却得到一条「安装浏览器扩展」的人工待办，
  任务没有完成。

## 根因

| Req | 根因 | 修复 |
|---|---|---|
| R1 | `capability` 层的只读白名单 `is_review_safe_named_tool` 里没有 `web_fetch`，于是在允许工具最少的薄会话里，唯一可见的「读网页」工具是 `browser_session`，而它在未装扩展时必然失败 | 把 `web_fetch` 加入只读白名单，并补测试断言 ReviewOnly 的 schema 里确实带 `web_fetch` |
| R2 | 模型只看到 `browser_session`，因此选择它 | 同上；`web_fetch` 的描述本来就是「默认读 URL 的方式」，可见后即成为首选 |
| R3 | `browser_session` 描述里的「prefer `attach`」把公开网页也导向了必须配对的 attach | 描述改为：只有在需要用户本人已登录的浏览器时才 `attach`，用户只给公开 URL 时用 `web_fetch`；`attach` 失败提示补一句「公开网页不需要扩展，可直接用 web_fetch」 |
| R4 | `is_forge_mutation_segment` 把 `git branch` 整个子命令列为变更，而它在只读探测判断之前短路，导致 `git branch` / `git branch --show-current` 这类纯列举被判成变更 | `git branch` 按参数分档：只有列举型 flag（`-a/-r/-v/-vv/--list/--show-current/--format=/--sort=/--merged/--contains/--points-at` 等）算查询；`-d/-D/-m/-M/-c/-C/-f/-u/--set-upstream-to/--unset-upstream/--edit-description` 或任何位置参数（创建分支）仍是变更 |

## 不变量

1. **只读会话必须能完成只读请求**：网页读取、仓库现状查询不得依赖「装浏览器扩展」这种前置人工动作。
2. **镜像边界不放松**：`git branch -D main`、`git branch new-feature` 等写操作仍然是 `Mutation`，测试里逐条断言。
3. `gh pr view` 等远端读取仍归 `Verification`（既有语义），只要不是 `Mutation` 就不会被只读门禁拒绝。

## 测试

- `codefactory-agent-core`：`m131b_git_branch_listing_is_a_read_only_probe`
  （列举形式 → `ReadOnly`；写形式 → `Mutation`；现场那条复合命令 → 非 `Mutation`）。
- `codefactory-agent-loop`：`m131b_review_turns_can_see_the_page_reader`
  （`tool_visible_for_capability(ReviewOnly, "web_fetch")` 为真、`capability_denial` 放行 `web_fetch`、ReviewOnly schema 含 `web_fetch`）。
