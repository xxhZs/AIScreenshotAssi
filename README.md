# Darling

Darling 是一个 macOS 全局悬浮的 AI 助手：你在任何应用里输入触发键，就会弹出一个轻量输入框；回车后生成一段**可直接粘贴**的文本，并自动回到你刚才的应用里完成输入。

适合的场景：
- 回微信/邮件/IM：一句话快速生成合适的回复
- 写文档/总结：根据屏幕内容继续写、改写、精炼
- 写代码/修报错：根据当前编辑器/错误提示给出下一步输出

## 特性

- 全局呼出：无需切回主窗口
- 直接可粘贴：默认不污染剪贴板（通过键盘事件输入）
- 屏幕上下文：可选开启截图，让 AI 根据“你正在做什么”来输出更贴合的内容

## 系统要求

- macOS（建议 13+）
- Node.js + npm
- Rust toolchain（Tauri）

首次使用需要在系统设置里授予权限（见下文）。

## 安装与运行（开发模式）

1) 安装依赖

```bash
npm install
```

2) 配置 `.env`

复制模板：

```bash
cp .env.example .env
```

在 `.env` 里至少配置一个文本模型（主模型）：
- `DARLING_LLM_KIND`（一般用 `openai_compat`）
- `DARLING_LLM_MODEL`
- `DARLING_LLM_API_KEY`
- `DARLING_LLM_BASE_URL`（如果你用代理/第三方 OpenAI 协议，就填它的 base url）

3) 启动

```bash
npm run tauri dev
```

## 权限设置（重要）

Darling 需要一些系统权限才能实现“全局呼出 + 自动输入”：

- **Accessibility（辅助功能）**：用于全局快捷键、读取选中文本/窗口信息
- **Input Monitoring（输入监控）**：用于拦截触发键
- （可选）**Screen Recording（屏幕录制）**：用于截图上下文

路径：System Settings → Privacy & Security → 对应条目里勾选 Darling。

## 使用方法

1) 在任意应用里输入触发键：`//`
2) 弹出输入框后输入你的指令，回车
3) Darling 会生成文本，并返回到你刚才的应用里把内容输入进去

一些例子：
- “帮我礼貌地拒绝他，别太生硬”
- “把我正在看的这段内容总结成三条”
- “根据这个报错给我下一步怎么改”

### 调试模式（可选）

在输入框里按 `Cmd+Shift+D` 会打开一个小面板，查看捕获到的上下文和本次运行信息（用于排查“为什么没带上上下文/截图”）。

## 屏幕上下文（可选）

如果你希望 AI “看懂你在干什么”，可以开启截图上下文：

在 `.env` 里设置：
- `DARLING_CTX_SCREENSHOT=1`

如果内容有滚动条、截图覆盖不全，可以开启“滚动多屏截图”（会短暂滚动页面再滚回）：
- `DARLING_CTX_SCROLL_CAPTURE=1`

### 截图理解模型（推荐与主模型分开）

为了让主模型保持“黑盒文本模型”，Darling 支持用一个单独的视觉模型把截图先转换成简短的「屏幕上下文」，再交给主模型做最终输出。

在 `.env` 里配置（示例）：
- `DARLING_VISION_EXTRACT=1`
- `DARLING_VISION_MODEL=gpt-5.2`
- `DARLING_VISION_API_KEY=...`
- `DARLING_VISION_BASE_URL=...`

## 打包发布

```bash
npm run tauri build
```

产物会在 Tauri 的 build 输出目录中生成。

## 安全与隐私

- 开启截图上下文意味着会截取屏幕并发送给你配置的“截图理解模型”服务端（如果你启用了该功能）。
- 建议只在你信任的 API / 代理上使用，并注意不要在敏感信息界面开启该功能。
