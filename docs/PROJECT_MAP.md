# Project Map — VideoTrans Tauri 目录结构全景图（背诵版）

> 用途：换模型/换环境后，照这张图直接背出当前项目结构。每个文件都标注了行数与一句话职责。
> 数据来源：实际文件系统扫描（2026-08-24），与 `REFACTOR_HANDOFF.md` §4 互相对照。

## 0. 数字总览（第一句就背这个）

| 维度 | 数字 |
|---|---|
| 后端 | `src-tauri/src/` 共 **18,338 行 Rust**，11 个命名空间目录 |
| 前端 | `src/` 共 **5,435 行 TS/Vue**（5 视图 + 7 组件 + 8 lib 文件） |
| Tauri 命令 | **31 个**（30 活跃 + 1 legacy `start_task`），IPC 名一个字母未变 |
| 测试 | 后端 `cargo test` **194 过 / 7 ignored**；前端 `npm test` **27 过**；`npm run e2e` 真机回归一键脚本（预检+容错 skip） |
| 推理 | `inference` cargo feature（默认开）；candle(Whisper) + ONNX Runtime(Supertonic/ZipVoice) |
| 磁盘 | `target/debug` 已瘦身至 ~2.8G（`debug=1`）；`.cargo/config.toml` jobs=2 勿删 |

## 1. 记忆口诀（50 字背骨架）

> 入口 main → lib 装配 → commands 三十一
> 业务 application 六件套，执行在 pipeline 四兄弟
> 引擎 engines 裸、适配 adapters 套，端口 ports 纯 trait
> 类型 domain、落盘 infra、调度 scheduler、预审 memcheck
> 旧路 legacy 已冻结，字幕工具散根级

## 2. 仓库根目录

```
videotrans-tauri/
├── src/                        # 前端 Vue3 + naive-ui + TS（5,435 行）
├── src-tauri/                  # 后端 Rust Tauri v2（见 §4）
├── docs/                       # 六份架构/需求文档（见 §7）
├── index.html / env.d.ts       # Vite 入口 / 类型声明
├── vite.config.ts              # Vite 配置（test.exclude 屏蔽 .refactor-backup）
├── tsconfig.json / tsconfig.node.json / tsconfig.web.json
├── package.json / package-lock.json
├── components.json             # naive-ui 按需组件清单
├── README.md / .gitignore / .vscode/
└── .refactor-backup/           # ⚠️ 唯一回滚网（src-tauri-src + src 副本），项目无 git，勿动
```

## 3. 前端 `src/`（5,435 行）

```
src/
├── main.ts            (14)  入口：createApp + 全局 error/unhandledrejection → log_frontend 落后端日志
├── App.vue           (111)  应用外壳：n-config-provider + AppSidebar + 按 store.currentPage 切换 5 视图
├── style.css         (164)  全局样式
│
├── views/                  # 5 个页面（对应左侧导航）
│   ├── HomePage.vue     (937)  工作台：视频选择(FileDrop)/源语言/目标配置/一键开始
│   ├── TasksPage.vue    (659)  任务队列：串行调度、统计卡片、任务卡片进度/日志
│   ├── SubtitlePage.vue (344)  字幕编辑：父级原文/各目标译文、导入SRT、导出
│   ├── ToolsPage.vue    (261)  媒体工具：ASS 样式预览、视频剪辑/分离/合成等 ffmpeg 工具
│   └── SettingsPage.vue (429)  设置：引擎字段按 engines.ts 数据驱动渲染
│
├── components/              # 7 个共享组件
│   ├── AppSidebar.vue     (185)  左侧导航栏
│   ├── ConfigFields.vue   (112)  引擎配置字段渲染（按 FieldDef 数据驱动）
│   ├── FileDrop.vue       (123)  视频拖拽/点击选取区
│   ├── Icon.vue           (110)  内联 SVG 图标集
│   ├── ApiTestButton.vue   (86)  API 连通性测试按钮（调 test_api_*）
│   ├── PageHeader.vue      (44)  页面标题栏
│   └── PathPicker.vue      (51)  路径输入 + 浏览按钮
│
└── lib/                     # 8 个逻辑文件
    ├── store.ts  (518)  模块级全局 store（切页不丢状态）+ runQueue 队列（MAX_CONCURRENT=2）
    ├── api.ts    (190)  Tauri invoke/event 封装：全部 31 个 IPC 的唯一前端入口
    ├── engines.ts(269)  引擎注册表（STT/TTS/翻译 三数组，数据驱动设置页；新引擎只加数据）
    ├── types.ts  (206)  前端类型（与后端 types.rs serde 对齐）
    ├── srt.ts     (64)  SRT 时间戳格式化/导出
    ├── ass.ts     (42)  ASS 字幕样式模型
    ├── langs.ts   (53)  语言选项（与 whisper 语言代码对齐，含 "auto" 哨兵）
    └── __tests__/
        ├── engines.test.ts (93)   引擎契约测试
        └── store.test.ts  (370)   队列调度测试
```

## 4. 后端 `src-tauri/` 构成

```
src-tauri/
├── Cargo.toml              # feature = ["inference"]；[profile.dev] debug=1；jobs 见 .cargo
├── build.rs                # 下载/恢复 onnxruntime.dll 1.28.0 到 target（含 deps/ 同步，cargo test 用）
├── tauri.conf.json         # 应用窗口/打包配置
├── capabilities/default.json  # 前端权限边界（fs/opener 等 scope）
├── .cargo/config.toml      # ⚠️ jobs=2（本机虚拟内存有限，勿删勿提）
├── assets/ + icons/ + gen/ # 静态资源 / 图标 / 生成目录
└── src/                    # 18,338 行 Rust，见 §5
```

## 5. 后端 `src-tauri/src/` 全景（背诵主图）

```
src/
│
├── main.rs            (5)   入口：Windows release 隐藏控制台 + videotrans_tauri_lib::run()
├── lib.rs           (157)   Composition root：mod 声明 + 配置加载 + Builder 装配 + invoke_handler 注册 31 命令
├── state.rs          (23)   运行中任务登记表 TASKS（句柄+取消令牌，全局唯一；只存运行态）
├── types.rs         (273)   AppConfig（含 zipvoice_* 字段）/ Segment / ProgressEvent / TaskConfig
├── logger.rs        (175)   日志双输出(stderr+按日文件) + panic hook 写 crash.log（必须在 Builder 前安装）
├── scheduler.rs     (339)   资源感知调度：Cost 声明 + CPU/API 准入 + RAII 租约（全应用唯一懂资源的地方）
├── memcheck.rs       (77)   commit 内存预审：STT 开工前按可用内存选 30s/15s 窗口或拒绝
├── audio_io.rs      (191)   音频流式窗口读取（STT 引擎共用，峰值内存与时长无关）
├── audio_align.rs    (99)   align_wav_to_duration：rubberband 限速对齐到目标时长
├── text_align.rs     (98)   align_text_to_segments：纯文本逐行匹配到字幕段
├── segments.rs      (106)   字幕段边界清洗（NaN/负值/end<=start 等坏段统一契约）
├── subtitle.rs      (103)   写 SRT 文件
├── subtitle_parse.rs (75)   解析 SRT（导入用）
├── tts_dub.rs       (193)   TTS 三引擎共用：时间轴组装 + 静音填补 + 流式写 dub.wav + i16 段读写（无门控）
├── e2e.rs           (306)   端到端流水线测试（需本地模型，--ignored；31s 跨窗回归用例）
│
├── commands/               # 8 文件 = 30 命令（见 §6 命令清单）
│   ├── mod.rs          (8)   模块声明
│   ├── config.rs     (157)   10 命令：配置读写/文件选择/文本读写/open_path/日志
│   ├── api_test.rs   (140)   2 命令：test_api_endpoint / test_api_reachable
│   ├── task.rs       (368)   3 命令：start_multi_target_task / cancel_task / cancel_child_task
│   ├── tasks.rs      (380)   8 命令：持久化任务 CRUD + 字幕段读写 + 方言
│   ├── media_tools.rs(184)   4 命令：match_text_to_srt / clip_video / separate_media / merge_video_audio
│   ├── runtime.rs    (159)   1 命令：get_runtime_info（ffmpeg/GPU/日志路径探测）+ RuntimeModelStatus
│   ├── realtime_stt.rs (70)  1 命令：transcribe_audio_chunk（实时录音分片）
│   └── diarization.rs(166)   1 命令：run_speaker_diarization（说话人分离）
│
├── application/             # 业务用例层（6 文件，编排 pipeline + domain + infra）
│   ├── mod.rs            (6)  模块声明
│   ├── pipeline_service.rs(504) 装配：注册各 StageExecutor 到 registry + registry.register 助手 reg()
│   ├── task_service.rs  (346)  持久化任务服务：create/recover/list，对接 task_store
│   ├── checkpoint.rs    (207)  错误恢复：TaskStoreCheckpoint + recover_task（断点续跑）
│   ├── subtitle_edit.rs (252)  字幕段读写（父级原文/目标译文，保存后失效逻辑）
│   ├── subtitle_import.rs(184) 导入 SRT 到目标任务
│   └── dialects.rs      (127)  方言配置加载（dialects.json 读写 + 内置方言）
│
├── pipeline/                # DAG 流水线（3 文件 + runner/ 5 文件）
│   ├── mod.rs            (5)  模块声明
│   ├── graph.rs        (285)  PipelineGraph：StageNode/NodeScope(Parent|Target)/拓扑排序
│   ├── registry.rs      (139)  StageRegistry：stage id → executor 注册表
│   ├── integration_tests.rs(274) 流水线集成测试
│   └── runner/              # 原 runner.rs(1758) 拆分，外部路径 crate::pipeline::runner::* 不变
│       ├── mod.rs      (15)    re-export 门面
│       ├── types.rs   (153)   RunScope/StageExecutor trait/StageRequest/ExecutionContext/CancelToken
│       ├── records.rs  (119)   StageRecord 读写辅助（pub(in runner) 私有可见性）
│       ├── executor.rs (733)  DAG 执行器：拓扑推进 + 检查点 + RAII 租约主作用域
│       └── tests.rs    (791)  runner 单元测试
│
├── engines/                 # 裸引擎：最小推理/IO 封装，**不依赖 ports**（3 域）
│   ├── mod.rs            (8)  命名空间说明
│   ├── stt/                # 4 引擎
│   │   ├── mod.rs         (8)
│   │   ├── sensevoice.rs (294)  SenseVoice：sherpa-onnx 绑定，int8 245MB，非自回归，token 级时间戳
│   │   ├── whisper_native.rs(545) candle Whisper：safetensors f32 常驻 3.2GB，流式分窗解码
│   │   ├── openai_api.rs (121)  OpenAI Whisper API（请求/响应双向日志，key 不落盘）
│   │   └── whisper_cli.rs (66)  whisper.cpp CLI 子进程
│   ├── translate/
│   │   ├── mod.rs         (3)
│   │   └── deepseek.rs   (227)  DeepSeek（OpenAI 兼容）字幕翻译
│   └── tts/
│       ├── mod.rs         (4)
│       └── supertonic/      # 唯一带子目录的引擎（ONNX Runtime 推理）
│           ├── mod.rs   (275)  合成入口 synthesize_segments*，对外唯一出口
│           ├── assets.rs(147)  31 语言 + 中文扩展资产校验（纯存在性检查）
│           └── helper.rs (816)  移植自上游官方运行时（已删 4 个死函数，仅本子树可见）
│
├── adapters/               # port 实现：包装 engines + infra，实现各域 StageExecutor（5 域）
│   ├── mod.rs           (5)
│   ├── stt/                # 4 文件
│   │   ├── mod.rs        (4)
│   │   ├── legacy.rs    (109)  ConfiguredSttEngine：按配置分发到 4 个 STT 引擎
│   │   ├── sensevoice.rs (95)  SenseVoiceEngine（SttEngine impl，sanitize_segments 出口）
│   │   └── stage.rs     (254)  SttStageExecutor：STT 阶段执行器
│   ├── translate/           # 2 文件
│   │   ├── mod.rs        (2)
│   │   ├── openai_compatible.rs(214) OpenAiCompatibleTranslator（DeepSeek 等）
│   │   └── stage.rs     (222)  TranslateStageExecutor
│   ├── tts/                 # 4 文件
│   │   ├── mod.rs        (6)
│   │   ├── cosyvoice3.rs (234)  CosyVoice3（本地/远程变体）
│   │   ├── supertonic.rs (117)  Supertonic 适配（segment_dir: Some 逐段 wav 契约）
│   │   ├── zipvoice.rs   (360)  ZipVoice：sherpa OfflineTts + preflight(6 必需文件) + 5 contract 测试
│   │   └── stage.rs     (226)  TtsStageExecutor
│   ├── separation/          # 2 文件
│   │   ├── mod.rs        (3)
│   │   ├── sherpa_uvr.rs (441)  SherpaUvrSeparator：C FFI（#![cfg(feature="inference")]）
│   │   └── stage.rs     (447)  SeparationStageExecutor
│   └── media/               # 3 文件
│       ├── mod.rs        (3)
│       ├── ffmpeg.rs    (315)  FfmpegMediaTool：probe / extract_stt_audio（MediaTool impl）
│       ├── stages.rs    (229)  MediaStageExecutor
│       └── output_stages.rs(523) FfmpegOutputStages：mix/SRT/final 输出阶段
│
├── domain/                 # 纯类型，无 IO（8 文件）
│   ├── mod.rs         (8)
│   ├── ids.rs        (45)   String newtype ID 族：TaskId/ChildTaskId/VariantId/StageId/ArtifactId
│   ├── variant.rs    (95)   TargetVariant（语言/方言/翻译风格/TTS 口音）
│   ├── dialect.rs    (65)   DialectSpec / LanguageDialectSpec / builtin_dialects
│   ├── task.rs      (126)   ParentTask/ChildTask + 状态机（ParentStatus/ChildStatus）
│   ├── config.rs    (166)   PipelineConfig/OutputConfig（默认值函数族）
│   ├── artifact.rs   (96)   ArtifactKind（probe/extract/STT/separation/translate/TTS/mix/SRT/final 九类）
│   ├── media.rs      (81)   SourceFingerprint/SourceVideo/MediaInfo
│   └── manifest.rs  (297)   TaskManifest/StageRecord/StageStatus/FallbackRecord（恢复核心）
│
├── ports/                 # 纯 trait，无实现（5 文件）
│   ├── mod.rs      (5)
│   ├── stt.rs    (151)   SttEngine + sanitize_segments
│   ├── tts.rs     (76)   TtsEngine + validate_tts_input + TtsOutput/TtsAlignment
│   ├── translator.rs (72) Translator + validate_translation
│   ├── separator.rs   (74) AudioSeparator + validate_separation_output
│   └── media_tool.rs  (60) MediaTool + validate_media_info
│
├── infra/                 # 落地设施（4 文件）
│   ├── mod.rs        (4)
│   ├── task_store.rs (769)  TaskStore：任务文档 JSON 持久化 + 索引重建
│   ├── artifact_store.rs(580) ArtifactStore：产物落盘 + staging(.tmp) + sha256 校验
│   ├── api_client.rs (453)  ApiClient/ApiRequest/ApiExecution（reqwest，含 Cost 分流）
│   └── diskcheck.rs  (82)   estimate_task_bytes 磁盘空间预审
│
└── legacy/                # 【冻结】仅紧急回滚，禁新代码依赖（3 文件）
    ├── mod.rs       (9)   冻结声明
    ├── command.rs  (121)  start_task（IPC 名保持兼容）
    ├── process.rs  (276)  旧单目标流程编排
    └── ffmpeg.rs   (312)  旧 ffmpeg 封装
```

## 6. 31 个 Tauri 命令清单（IPC 契约）

| 文件 | 个 | 命令 |
|---|---|---|
| `commands/config.rs` | 10 | check_ffmpeg、pick_onnx_model、pick_video_files、read_text_file、write_text_file、load_config、save_config、open_path、get_log_dir、log_frontend |
| `commands/api_test.rs` | 2 | test_api_endpoint、test_api_reachable |
| `commands/task.rs` | 3 | start_multi_target_task、cancel_task、cancel_child_task |
| `commands/tasks.rs` | 8 | create_persistent_task、list_persistent_tasks、load_persistent_task、ensure_uvr_model、load_dialect_specs、load_task_segments、save_task_segments、import_target_srt |
| `commands/media_tools.rs` | 4 | match_text_to_srt、clip_video、separate_media、merge_video_audio |
| `commands/runtime.rs` | 1 | get_runtime_info |
| `commands/realtime_stt.rs` | 1 | transcribe_audio_chunk |
| `commands/diarization.rs` | 1 | run_speaker_diarization |
| `legacy/command.rs` | 1 | start_task（冻结兼容入口） |
| **合计** | **31** | 前端只认这些 IPC 名，改模块路径不影响前端 |

## 7. 依赖方向（架构记忆锚）

```
commands ──► application ──► pipeline ──► adapters ──► engines     (裸推理/IO)
   │              │              │            │
   │              ▼              ▼            ▼
   └──────────► domain + ports(纯类型/trait) ◄──┘
   └──────────► infra(落盘/API/磁盘) ◄────────────┘
   └──────────► state/scheduler/memcheck(运行态/资源) ◄── pipeline(租约) 与 adapters(成本)
```

- **依赖方向铁律**：`engines` 不依赖 `ports`；`adapters` 依赖 `ports + engines`；`domain/ports` 零依赖；`infra` 只依赖 domain。
- **资源调度**：pipeline 只声明 `Cost`，scheduler 决定准入——pipeline 不认识信号量，scheduler 不认识流水线。
- **回滚衔接**：`legacy` 仅 `command.rs:start_task` 暴露为 IPC，内部 process/ffmpeg 冻结。

## 8. docs/ 六份文档

| 文件 | 内容 |
|---|---|
| `ARCHITECTURE.md` | **当前架构唯一权威**（分层图 + 依赖规则 + 模块索引） |
| 各模块 `ARCHITECTURE.md` | pipeline / adapters / engines / application / src-lib 五份模块级架构图与说明 |
| `FRONTEND.md` | 已归档（早期前端需求草稿，被 `src/lib/ARCHITECTURE.md` 取代） |
| `FUNCTION_CHECKLIST.md` | **需求跟踪源**（基础/次要/待规划/待对齐分区；改主流程必须先对照） |
| `REFACTOR_HANDOFF.md` | 会话交接（当前状态 + 门禁基线 + 操作手册） |
| `TASK_PIPELINE_ARCHITECTURE.md` | 流水线设计 |
| `ARCHITECTURE_REVIEW_10_ROUNDS.md` | 十轮架构评审记录 |

## 9. 关键路径与契约（背诵点）

```
本地模型路径（本机已有，无需下载）：
  Whisper:   E:\projects\text2voices\CosyVoice\pretrained_models\whisper-large-v3-turbo
  Supertonic:E:\projects\pyvideotrans-3.98\Supertone\supertonic-3
  ZipVoice:  E:\projects\text2voices\sherpa-onnx-zipvoice-distill-int8-zh-en-emilia\
             （encoder/decoder.int8.onnx + tokens.txt + lexicon.txt + vocos_24khz.onnx + espeak-ng-data/）

日志：%LOCALAPPDATA%\videotrans\logs\（videotrans-YYYYMMDD.log / failures.jsonl / crash.log）
门禁：cargo test = 194 过 7 ignored；npm test = 27 过；cargo check 双模式（默认 0 警告 / 无推理 ~49 既有依赖警告）
```