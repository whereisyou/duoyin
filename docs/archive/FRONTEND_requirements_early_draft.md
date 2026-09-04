# VideoTrans 前端功能需求文档

> 本文档记录 VideoTrans 桌面应用的前端功能需求、页面设计、组件规划与待办事项。
> 技术栈：Vue 3 + TypeScript + Naive UI + Vite

---

## 页面总览

| 页面 | 路由 | 状态 | 说明 |
|------|------|------|------|
| 首页 | `/` | ✅ 已完成 | 文件选择、语言选择、任务执行、进度展示、字幕预览 |
| 设置页 | `/settings` | ✅ 已完成 | API Key 配置、输出目录配置（含文件夹浏览） |

---

## 首页

### 功能列表

| 功能 | 状态 | 说明 |
|------|------|------|
| FFmpeg 环境检测 | ✅ | 启动时自动检测，未安装时显示错误提示 |
| 文件选择（点击） | ✅ | 点击触发 Tauri 原生文件对话框，选择视频文件 |
| 文件选择（拖拽） | ✅ | 拖拽文件到窗口，监听 `tauri-drag-drop` 事件获取路径 |
| 源语言选择 | ✅ | n-select 下拉选择：中文/英语/日语/韩语/法语/德语 |
| 目标语言选择 | ✅ | 自动过滤已选的源语言 |
| 开始处理按钮 | ✅ | 禁用态 + loading 态 |
| 取消按钮 | ✅ | 处理中显示，调用 cancelTask |
| 进度条显示 | ✅ | n-progress，显示百分比 + 当前步骤名称 |
| 字幕预览 | ✅ | n-table 展示原文/译文/时间，翻译结果对接 |
| 错误提示 | ✅ | n-alert，可关闭 |

### 流程

```
用户拖拽/选择文件
    ↓
选择源语言 + 目标语言
    ↓
点击"开始处理"
    ↓
检查 API Key 配置（调用 loadConfig）
    ↓
调用 startTask（返回 taskId）
    ↓
监听 task:{taskId} 事件
    ↓
实时更新进度条 → 完成 / 错误
    ↓
展示字幕预览表格
```

### 涉及组件

- `FileDrop.vue` — 文件选择区
- `n-select` — 语言下拉框
- `n-button` — 开始/取消按钮
- `n-progress` — 进度条
- `n-alert` — 错误/警告提示
- `n-table` — 字幕预览表格

---

## 设置页

### 功能列表

| 功能 | 状态 | 说明 |
|------|------|------|
| STT 配置 | ✅ | OpenAI API Key（密码框 + 显示切换） |
| 翻译配置 | ✅ | DeepSeek API Key（密码框）、模型名 |
| TTS 配置 | ✅ | CosyVoice API Key、API 地址（预留） |
| 输出配置 | ✅ | 输入框 + "浏览"按钮打开文件夹对话框 |
| 保存配置 | ✅ | 调用 saveConfig，useMessage 成功/失败提示 |
| 加载配置 | ✅ | onMounted 时调用 loadConfig |

### 涉及组件

- `n-card` — 分区卡片
- `n-input` — 输入框（普通 + password）
- `n-button` — 保存/浏览按钮
- `useMessage` — 操作反馈提示

---

## 组件清单

### 通用组件

| 组件 | 文件 | 状态 | 说明 |
|------|------|------|------|
| NavBar | `NavBar.vue` | ✅ | 导航栏，首页/设置切换 |
| FileDrop | `FileDrop.vue` | ✅ | 文件拖拽/点击选择，监听 `tauri-drag-drop` 事件 |

### Naive UI 组件（按需引入）

| 组件 | 使用页面 | 说明 |
|------|---------|------|
| `n-config-provider` | App.vue | 全局主题配置 |
| `n-message-provider` | App.vue | 消息提示 |
| `n-button` | NavBar, HomePage, SettingsPage | 按钮 |
| `n-select` | HomePage | 下拉选择 |
| `n-progress` | HomePage | 进度条 |
| `n-alert` | HomePage | 警告/错误提示 |
| `n-table` | HomePage | 字幕预览表格 |
| `n-card` | SettingsPage | 卡片容器 |
| `n-input` | SettingsPage | 输入框 |

---

## Rust IPC 接口（前后端契约）

### 前端命令调用

| 前端函数 | 后端命令 | 参数 | 返回值 | 说明 |
|---------|---------|------|--------|------|
| `pickVideoFile()` | `pick_video_file` | 无 | `Option<String>` | 打开文件选择对话框 |
| `checkFfmpeg()` | `check_ffmpeg` | 无 | `String` | 检查 ffmpeg 是否可用 |
| `startTask(config)` | `start_task` | `TaskConfig` | `String` (taskId) | 启动处理任务 |
| `cancelTask(id)` | `cancel_task` | `id: String` | `()` | 取消任务 |
| `loadConfig()` | `load_config` | 无 | `AppConfig` | 读取配置 |
| `saveConfig(config)` | `save_config` | `config: AppConfig` | `()` | 保存配置 |

### 前端事件监听

| 事件名 | 类型 | 说明 |
|--------|------|------|
| `task:{taskId}` | `ProgressEvent` | 任务进度更新 |
| `tauri-drag-drop` | `{ paths: string[] }` | 文件拖拽事件（Rust 端 `on_window_event` 转发） |

### 数据类型

```typescript
interface TaskConfig {
  video: string
  source_lang: string
  target_lang: string
}

interface AppConfig {
  openai_key: string
  deepseek_key: string
  deepseek_model: string
  cosyvoice_key: string
  cosyvoice_url: string
  output_dir: string
}

interface ProgressEvent {
  step: string       // extract_audio / stt / translate / split_audio / srt / done
  progress: number   // 0-100
  status: string     // running / done / error
  error?: string
}

interface SubtitleSegment {
  idx: number
  start: number      // 秒
  end: number
  text: string
  translated?: string
}
```

---

## 待办事项

### 功能

- [ ] **批量处理** — 多个视频文件队列处理
- [ ] **配音功能** — 集成 CosyVoice TTS
- [ ] **处理完成通知** — 任务完成后弹系统通知

### 优化

- [ ] **API Key 输入校验** — 检查格式（sk- 开头等）
- [ ] **深色模式** — Naive UI 内置支持，但需要添加切换开关
- [ ] **性能优化** — vue-tsc 在 Windows 上内存溢出，需排查（当前 `npm run build` 跳过 typecheck，单独 `npm run typecheck` 可运行）
- [ ] **构建缓存** — 配置 esbuild 缓存减少 OOM 概率