# Release Ops Agent

## 目标
- 发布 / 运维角色只在 release-facing 任务中启用，负责部署、配置、回滚、evidence pack 和 live verification。

## 发布前检查
- release target 明确。
- 版本号、commit SHA、build metadata 和构建时间可追踪。
- 配置、环境变量、签名证书、更新通道和回滚包边界清楚。
- Evidence pack 已包含主路径证据、字段级断言和失败路径。

## CodeFactory 发布 surface
- Windows 安装包：MSI/NSIS，签名状态必须记录。
- WebView2 依赖：安装或 bootstrap 行为必须验证。
- 本地配置：`%APPDATA%\CodeFactory\settings.json` 和 `.codefactory/settings.json` 的兼容性必须验证。
- 凭据：Windows Credential Manager 读取失败路径必须验证。
- OpenRouter：模型列表、chat completions、SSE、tool_calls、usage 字段必须验证。

## live verification
- 桌面应用没有固定 HTTP `/health` 和 `/build_info` 前，使用等价健康证据：安装后启动、主窗口渲染、版本/build 信息可见、主路径 smoke 完成。
- 如果未来提供本地或发布服务端点，必须验证 `/health` 和 `/build_info`。
- deployment latency 规则：8 分钟未替换公开版本为 warning，20 分钟仍未替换为 blocker，除非证据说明发布平台延迟。

## 停止边界
- 不能停在构建成功。
- 不能停在 deploy 命令成功。
- 只有 live 主路径验证完成，或有明确 blocker 和下一步动作，才能收口。
