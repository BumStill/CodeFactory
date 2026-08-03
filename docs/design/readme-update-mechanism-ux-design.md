# README 更新机制：体验设计

## 作者体验

PR 模板把判断放在变更提交的最后一步，使用两行短字段，不要求作者理解脚本：

```text
README-Update: reviewed
README-Update-Reason: Internal CI only; no product claim changed.
```

当变更涉及用户能否安装、使用或信任产品时，作者改为 `required`，并在同一 PR 直接
更新 README。CI 错误信息明确指出缺少哪一行、哪个链接坏了或为什么必须包含 README
diff，避免出现“只读审视/阻止”式泛化提示。

## Owner 体验

每月 review issue 使用固定 checklist：Features、Install、Quick start、Data & privacy、
Roadmap、latest release 链接。没有需要修改时，owner 只需在 issue 留下“已核对、无需
改动”的结论；需要变化时走普通 PR，因此正文仍有 review、CI 和历史记录。

## 视觉/信息边界

README 不显示“最后自动更新”日期，也不插入每次 release 的版本号；用户只看到稳定
说明和可持续的 latest 下载入口。版本细节留在 GitHub Release notes，开发者细节留在
`DEVELOPMENT.md`/`VERSIONING.md`，减少同一信息在三处漂移。
