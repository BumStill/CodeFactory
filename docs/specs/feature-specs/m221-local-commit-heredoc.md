# M221：在本会话工作区里用 heredoc 写提交说明的本地 commit 仍然要审批（M219 遗漏）

## 现场（11-04 02:2x，v1.91.73 真机验收 M219）
- 信任模式的测试会话 7efdbdd7，工作目录就是它自己的 execution-workspace，执行了：
  `git add -A && git commit -q -F - <<'MSG'` + 一行说明 + `MSG` + `git log --oneline -1`
- 结果被 `trusted.heredoc.unproved` 拦下，原因是「heredoc 之前的 Git 不是只读：git add -A」。
- M219 已经修好了同一次验收里的另外几种写法：相对路径 cd 后跟 heredoc、heredoc 前串了只读命令、只读 git。往 /etc 写文件也正确要求了审批。只剩这一类。
- 按 10-20 的两级权限决定（M130），本地 commit 应该随写权限直接放行，远端动作才需要另外审批。M219 之前，M219 自己的会话也被这条拦过一次（`git add … && git commit -q -F - <<'MSG' && git push -q origin HEAD`）。

## 要求
- R1：heredoc 的消费者如果是 `git commit -F -` / `git commit --file=-` / `git tag -F -`，heredoc 正文只是提交说明这类数据，不是要执行的脚本，不应该走「解释器只访问工作区」的证明。整条命令按里面的 git 段落分别判断：工作区内的 `git add`/`git commit`/`git tag`（本地）随写权限放行；`git push` 等远端动作继续按 M208/M130 的规则判断。
- R2：对 `cat <<EOF | git commit -F -`、`git commit -F - <<EOF` 这两种写法一视同仁。
- R3：回归测试：上面的原始命令放行；同一种写法后面跟 `&& git push origin HEAD` 时，按交付授权判断（有授权就放行，没有就审批）；`git -C /别的仓库 commit -F - <<EOF` 仍然要审批。
- R4（补充现场样本，11-04）：heredoc 识别必须先按 shell 引号规则切词，引号内的 `<<` 只是参数，不是 heredoc。只读命令 `cd "<本工作区>" && sed -n 2680,2770p … && grep -n "heredoc\|<<" … | head -30` 曾被判为「heredoc 缺少换行」而要求审批；这类命令不应进入 heredoc 门禁。

## 验收
- 真机：开一个信任模式会话，原样执行上面的命令，不出现审批，并且产生本地提交。
- 真机验收由编排方在发版安装后完成（任务模板「真实应用验收的分工」）；会话内证据为权限门禁集成测试（真实产生本地提交）+ 单元测试 + 场景门禁。

## 决策（实现层，不改变上面任何要求）
- 判定归口仍在 `trusted_git::python_heredoc_proof`：新增「数据型消费者」类别，正文不再做解释器包含性证明。
- 数据型消费者的正文字面写入 Git 对象（提交/标签说明），不执行、不当路径解析；因此只保留「正文之后不得再有 shell 内容」这条结构约束。
- 前缀段落从「只读」放宽为「只读 或 本工作区内的本地 Git 写动词（add/commit/tag）」；`git -C <工作区外>`、`--git-dir`、`--work-tree` 一律继续要求审批。
- 远端动作不受本条影响：permission gateway 的 heredoc 分支在命令仍含远端动作时不再直接放行，而是继续走 M208/M130 的交付授权判断。

## Requirements Traceability

| Req ID | 要求 | 最低证据 |
| --- | --- | --- |
| R1 | `git commit -F -` / `git commit --file=-` / `git tag -F -` 的 heredoc 正文是数据；本工作区内的 `git add`/`git commit`/`git tag` 随写权限放行 | `trusted_git::m221_git_data_heredoc` 单测 + permission_gateway 集成测试真实产生本地提交 |
| R2 | `cat <<EOF \| git commit -F -` 与 `git commit -F - <<EOF` 同等对待 | 同上，两种写法逐条断言 `Allow` |
| R3 | 原始命令放行；带 `&& git push origin HEAD` 时按交付授权判断；`git -C <别的仓库> commit -F -` 仍审批 | permission_gateway 集成测试：`delivery_authorized=1` 放行、`=0` 生成审批；`-C` 越界产生 `permission_intents` 记录 |
| R4 | 引号内的 `<<` 不是 heredoc，只读探针不进 heredoc 门禁 | `trusted_git::m221b_quoted_double_angle` 单测 + permission_gateway 集成测试（现场原始命令判定 `Allow`、零审批） |

## Applicable Harnesses

| Harness | 适用理由 | 覆盖方式 |
| --- | --- | --- |
| Spec Harness | 权限判定行为变更 | 本规格 + Req ID ↔ 测试映射 |
| Compatibility Harness | 权限策略属于兼容面（旧写法不得退化） | `m215_trusted_heredoc_and_read_only_git`、`m198` 家族回归测试全部保持通过 |
| Payload Harness | heredoc 正文是输入载荷 | 数据型正文结构性约束（正文后不得有 shell 内容）+ 脚本型正文仍做包含性证明 |
| Viewport Harness | 不涉及 UI | 不适用：无界面改动 |

## 测试矩阵

| 场景 | 期望 | 测试 |
| --- | --- | --- |
| `git add -A && git commit -q -F - <<'MSG'`（工作区内） | 放行并真实产生提交 | gateway 集成测试 |
| `cat <<'MSG' \| git commit -F -`（工作区内） | 放行 | gateway 集成测试 + 单测 |
| `git commit --file=- <<'MSG'` | 放行 | 单测 |
| `git tag -a v1 -F - <<'MSG'` | 放行 | 单测 |
| 同上命令 + `&& git push origin HEAD`，有交付授权 | 放行 | gateway 集成测试 |
| 同上命令 + `&& git push origin HEAD`，无交付授权 | 审批 | gateway 集成测试 |
| `git -C <工作区外> commit -F - <<'MSG'` | 审批 | gateway 集成测试 + 单测 |
| `git add -A && python3 - <<'PY'`（正文是脚本） | 仍走解释器证明 | 单测 |
| `git reset --hard … ; git commit -F - <<'MSG'` | 审批 | 单测 |
| heredoc 正文之后再有 shell 内容 | 审批 | 单测 |
| `grep -n "heredoc\\|<<" file`、`rg --fixed-strings "<<" .`、`echo "use <<EOF"` | 不进 heredoc 门禁，只读即放行 | 单测 + 网关现场样本 |
| 现场只读探针 `cd "<本工作区>" && sed -n … && grep -n "heredoc\\|<<" … \| head -30` | `Allow`，零审批 | 网关集成测试 |
| 真 heredoc（`cat <<'EOF'`、`git commit -F - <<'MSG'`） | 仍按 heredoc 处理 | 单测 |
