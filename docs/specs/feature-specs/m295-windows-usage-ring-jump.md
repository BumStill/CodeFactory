# M295：Windows 上点聊天框右下角用量圈，圈会瞬移到左边且不弹出详情框

## 现场（用户 2026-11-24 报告，11-27 再次催问；macOS 正常）
- Windows：点击输入框右下角的上下文/用量圈，圈本身跳到左边，详情框没有像 mac 那样原地弹出。
- macOS：原地弹出详情框。

## 已知线索（编排方只读，origin/main）
- 组件 `src/components/ContextUsageBar.tsx`：按钮约 253–266 行 `setDetailOpen` 切换；详情框 `createPortal` 到 body，`position: fixed`，坐标由 trigger 的 `getBoundingClientRect()` 计算（约 180–200 行，`left = min(triggerRect.right - width, innerWidth - width - padding)`，`top = triggerRect.top - visibleHeight - padding`），监听 resize/scroll 重算。放置测试 `src/pages/Workspace/WorkspacePage.usageBarPlacement.test.tsx`。
- 推测方向（需核实，不要只猜）：Windows 经典滚动条占宽度，开框时某容器出现/消失滚动条导致布局重排把圈挤动；高 DPI（125%/150% 缩放）下 rect 与 innerWidth 不一致；开框时按钮内容/宽度变化；首帧测量在布局完成前；focus 导致滚动；portal 计算出负 top 被夹到 padding 而遮盖/跳动。

## 规格
- R1：在 Windows 与 macOS 上，点击用量圈：圈位置不变（像素级），详情框在圈附近弹出并完整可见；再次点击或点关闭收起；窗口缩放/滚动时跟随。
- R2：查清 Windows 上「圈瞬移」的真实原因并写进规格（给 file:line 与复现条件），修根因，不加针对平台的硬编码偏移。
- R3：测试（先红后绿）：headless 真浏览器验收页模拟 Windows 条件——强制占位滚动条（如 `::-webkit-scrollbar { width: 17px }` 并让容器可滚动）、设备缩放 1.25/1.5、窄窗口；断言点击前后圈的 boundingClientRect 不变、详情框可见且在视口内。不得只用 jsdom。
- R4：PR 写明「Windows 真机需用户回填」的具体场景：100%/125%/150% 缩放下点圈，圈不动、框弹出。

## 根因与实现说明
- 根因：`src/components/ContextUsageBar.tsx` 中详情 portal 打开后的 focus effect（当前约 202–205 行）调用无参数 `HTMLElement.focus()`。在 Windows WebView2 的经典占位滚动条、窄窗口与 125%/150% 缩放组合下，新挂载且首帧仍位于初始 fixed 坐标的 portal 会触发浏览器 scroll-into-view；滚动祖先随之移动 composer，表现为右下角用量圈瞬移，且详情框未稳定停在圈附近。macOS overlay scrollbar 不占布局宽度，通常不触发该路径。
- 修复：focus 详情框时传入标准 `preventScroll: true`，保留键盘焦点与 Escape/关闭行为，同时阻止 WebView2 为 fixed portal 改变滚动位置；不使用平台判断或硬编码坐标偏移。
- 复现与验证：`src/acceptance/contextUsageRing.tsx` 在真实浏览器验收页强制 17px 占位滚动条，并模拟 WebView2 对未声明 `preventScroll` 的 portal focus 滚动；`scripts/verify-context-usage-ring-headless.mjs` 在 1.25/1.5 device scale 与窄视口下比较点击前后 ring rect，检查 portal 视口边界、锚定、滚动/缩放跟随及两种关闭方式。

## 约束
改文件只用 edit_file / write_file。不改信任根；场景缺口只追加 `docs/testing/scenario-mappings.json`。不碰凭据、settings.json、其他会话 PR，不动任何 merge-queue: hold 标签。规格文件在工作区外，用 bash cat 只读查看。

## 交付
PR 正文 `R | 是否完成 | 证据` 列 R1–R4；默认紧急度（不写 Release-Urgency 行）。
