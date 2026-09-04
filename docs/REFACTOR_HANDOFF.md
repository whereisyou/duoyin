# Handoff — VideoTrans Tauri 后端重构 + 优化（当前状态快照）

> 交接文档：新会话从这里继续。**先读本文件**（含当前状态、边界、操作手册），架构细节看 `docs/ARCHITECTURE.md`（全局）+ 各模块文件夹内 `ARCHITECTURE.md`（细节）。

---

## 0. 当前状态（一句话）

**全部既定工作已完成且全绿。** 结构重构（S1–S6）+ 精简/性能（S8）+ 开箱收口（2026-08-25：测试补全、模型默认值、warning 清零、默认引擎修复）已收官，处于「待用户真机验收」状态。

**最新门禁基线（本程实测）**：

| 检查 | 结果 |
|---|---|
| `cargo test -j 1`（默认 inference） | **204 passed / 0 failed / 7 ignored**（基线 194 + 10 新增：delete_task×1、默认路径×2、翻译重试×2、voice_ref×3、tts 参考注入×2） |
| `cargo check -j 1`（默认） | 通过，**0 warning** |
| `cargo check --no-default-features -j 1` | 通过（~49 既有依赖警告） |
| `npm run test` | **27 passed** |
| `npx vue-tsc --noEmit` / `npx vite build` | 均通过 |
| `npm run e2e`（真机回归，`scripts/e2e.mjs`） | 预检 7 项 + **7/7 真机用例通过**（~21s） |

> 门禁纪律：任何后续改动后须保持 `cargo test`=204/7、`npm test`=27 全绿。若 npm test 变 54，说明 `vite.config.ts` 的 `test.exclude: ['**/.refactor-backup/**']` 失效。

**存储清理（2026-08，已完成）**：`target/debug` 曾达 28G。① 删 `incremental/`（-12G，可再生缓存）；② `Cargo.toml [profile.dev]` 加 **`debug = 1`**（只留行号表，削减 ML 大依赖调试符号）+ 全量重编译。期间遇到一次 rustc `0xc0000005` 偶发崩溃（全量并行重编译内存/commit 压力所致，与 debug=1 无关）→ 单 crate 隔离编译正常，改 `cargo test -j 1`（单线程降峰值）即过，**190 测试全过**。最终 **`target/debug` 28G → 2.8G**（deps 13G→2.2G、incremental 12G→354M、build 926M→205M），onnxruntime.dll 由本地 zip 离线恢复。file:line 行号保留；如需完整调试信息改回 `debug = 2` 重编。

---

## 1. 任务目标（已完成）

对 `E:\projects\pyvideotrans-3.98\videotrans-tauri` 的 Rust 后端（原 18,421 行）做结构优化 + 精简/性能优化，**功能零回归**（31 个 Tauri 命令 IPC 名一个字母未改、测试语义不变、legacy 回滚路径有效），并补全 ZipVoice TTS 后端。前端 5,271 行本轮未动。

**架构决策（已定稿，勿重开）**：4 模型会诊 + verifier 锦标赛（t18820-1）→ **C1：engines/ 命名空间 + legacy/ 隔离不删 + 分切片推进**。

---

## 2. 已完成工作（两阶段）

### 阶段一：结构重构（S1–S6）

- **S1 死代码删除**：删 `stt_whisper_candle.rs` / `api_cosyvoice.rs` / `api_whisper_rs.rs`（均零引用）。
- **S2 commands 层拆分**：`lib.rs` 915→**168 行**（composition root：mod 声明 + `run()`）。16 个内联命令移入 `commands/{config,api_test,task}.rs`；`RunningTask`+`TASKS` → `state.rs`。invoke_handler 实 **31 条**（16 内联移出 + 15 既有）。
- **S3 runner.rs 拆分**：`pipeline/runner.rs`(1758) → `pipeline/runner/{mod,types,records,executor,tests}.rs`，外部 `crate::pipeline::runner::*` 路径零变化。关键：`RunScope::node_key` 等内部件用 `pub(in crate::pipeline::runner)`（**不升 pub(crate)**）。
- **S4 引擎归位**：crate 根 8 引擎文件 → `engines/{stt,translate,tts}/`。supertonic 拆 `tts/supertonic/{mod,assets}.rs`；feature 门控下沉到各 mod.rs；8 个消费文件 sed 改 `crate::旧::` → `crate::engines::新::`。
- **S5 legacy/ 隔离**：`process.rs` + 根 `ffmpeg.rs` + `start_task` → `legacy/{process,ffmpeg,command}.rs`（**冻结，仅紧急回滚**；`e2e.rs` 改用 `adapters::media::FfmpegMediaTool::extract_stt_audio`）。`FUNCTION_CHECKLIST.md` §5 已标注冻结 + 移除标准。
- **S6 ZipVoice 补全**：根因 = 后端 `types.rs` AppConfig 缺 zipvoice 字段（serde 静默丢弃）。已补 `zipvoice_dir/prompt_wav/prompt_text/zipvoice_num_threads`；新增 `adapters/tts/zipvoice.rs`（包 sherpa-onnx `OfflineTts`）+ register_tts 分支 + runtime 状态 + preflight + 5 contract 测试 + 1 真机 e2e。**真机验证通过**（24000Hz 真实出声）。

### 阶段二：代码精简 / 性能 / 死代码（S8，verifier 锦标赛选定路线）

- **TTS 三引擎重复收敛**（t3208-1 胜者 C）：cosyvoice3/zipvoice/supertonic 重复的「时间轴对齐 + dub.wav 流式 + 静音填补 + rubberband」提取为 crate 根无门控模块 **`src/tts_dub.rs`**（`TimelineWriter` + `to_i16` + `write/read_segment_wav` + `align_i16_to_duration`）。删 100+ 行重复，时间轴逻辑与原实现**逐字节等价**；supertonic 每段 wav 契约（`segment_dir: Some`）不动。
- **微性能**：`audio_io::read_window` 单声道去 `mix_mono` 的 ~2MB/窗克隆；`TimelineWriter` 分块静音。
- **SIMD 诚实否决**：瓶颈在 ONNX 推理 + ffmpeg，非 glue 循环；`mix_mono` 真实路径（16kHz 单声道）= 空操作。不做手写 SIMD。
- **register 样板收敛**：`pipeline_service.rs` 的 `registry.register(...).map_err(debug_error)?` ×10 → helper `reg()`。
- **helper.rs 死代码删除**（t3208-2 选定 D 后被用户改判）：`engines/tts/supertonic/helper.rs` 删 4 个零调用死函数（`timer`/`sanitize_filename`/`TextToSpeech::batch`/`load_text_to_speech`）+ 摘掉 `#![allow(dead_code)]`，**890→816 行**。

### 阶段三：开箱收口（2026-08-25 会话，verifier 四赛四胜）

- **测试补全**（t3208-3）：新增 4 测试（`pipeline_config_from_app` 接线 ×3 + 真实 ffmpeg 混流→成片冒烟 ×1）；**测试日志隔离**（`logger.rs` 在 `cfg!(test)` 下重定向 `%TEMP%/videotrans-test-logs`，failures.jsonl 不再被 FakeExecutor 假记录污染）；**`npm run e2e`**（`scripts/e2e.mjs`）一键真机回归。
- **真机 Bug 修复**（t3208-4）：e2e 抓到 ORT C++ bad_alloc 穿透 extern "C" abort（0xc0000409，低 commit 内存下）→ `e2e.rs` 三个 TTS 用例加 `need_mem!` commit 预审宏。生产路径不动（scheduler::admit 已保护）。
- **模型路径默认值**（t3208-5）：`engines.ts` DEFAULT_CONFIG 填入 4 个本机模型路径 + ZipVoice 参考音频（`test_wavs/news-female.wav` + 转写）；README 模型路径速查表；契约测试改为「路径全空才未就绪」。**Supertonic 中文缺 `*_zh.onnx` 扩展 → 中文目标用 ZipVoice**（README 已标注）。
- **warning 清零**（t3208-6）：37 → 0。删真死 + `#[allow(dead_code)]` 标注「仅测试在用/预留能力/安全纵深」+ 4 类未接线能力登记 `FUNCTION_CHECKLIST.md §3`；`PipelineError` 断链用 `pub(in crate::pipeline::runner)` 子树级 re-export 收口。
- **STT 默认引擎修复**（t11144-1）：默认 `whisper_native`（3900MB 必被 memcheck 拒）→ `sensevoice`（1200MB 开箱可过），前后端 + 测试断言三处对齐。
- **文档**：`docs/PROJECT_MAP.md` 新建（背诵版目录图）；README 重写（`npm run tauri dev` 置顶 + 测试三档 + 模型速查表）。

**用户启动后必做 2 件事**（外部依赖）：① 设置页填 DeepSeek API Key；② 中文目标 TTS 选 ZipVoice。

### 阶段四：验收发现 bug 修复（2026-08-25 会话，verifier t9288-1 胜者包 A）

- **Bug1 删除任务重启复活**：根因 = 前端 `removeTaskItem`/`clearFinishedTasks` 只做内存 splice，后端无删除命令，目录+索引原封不动。修复 = 后端新增 `delete_persistent_task` 命令（TaskStore::delete_task 删目录 + 原子剔除 index.json 条目，幂等；TaskService 包装；运行中任务拒绝）+ 前端先 invoke 成功再本地 splice，失败弹提示不删。
- **Bug2 TTS 误报「需要配置」**：根因 = 后端 `AppConfig::default()` 模型路径为空串（t3208-5 只填了前端 DEFAULT_CONFIG），旧 config.json 空字段覆盖前端默认。修复 = 6 个默认路径走 serde default + `normalize_defaults()` 加载回填；前端 merge 不动（包 B 纯前端修法被否决：start_multi_target_task 运行时读后端 state，前端修不到）。
- **顺带**：`runner/mod.rs` L24 子树 re-export 补 `#[allow(unused_imports)]`（PipelineError/StageRunResult 仅测试消费，lib check 恢复 0 warning）。

### 阶段五：真机验收第二批（2026-08-25 会话，verifier t27348-1 胜者候选 1）

- **翻译 IncompleteResult 整任务失败**：translate stage 无重试，DeepSeek 偶发截断/漏段即 Failed。修复 = 重试 3 次（退避 0.8s/1.6s）→ 仍 IncompleteResult 时原文回填译文 + `ExecutionOutcome::Degraded`（TTS 照常跑、manifest 标记、可重试）。
- **STT 开头幻觉字幕**（实测：静音开头识别出「在古老纤细中发现了有机分子 0.0-4.14s」）：根因 = SenseVoice 无 VAD 门控。修复 = `silero_vad.onnx` 放入模型目录即启用 Silero VAD（VoiceActivityDetector 每 30s 窗先切语音段再识别，开头静音/背景音乐不再产生字幕；缺失文件时行为不变向后兼容）。模型目录现为 `E:/projects/test2voices_backup/sense-voice-int8/`（含 silero_vad.onnx）。
- **原视频音色克隆**（用户诉求「用原视频语音作样本」）：`tts_use_video_prompt` 开关（高级设置）→ TTS stage 自动提取参考段（voice_ref：最长 3~20s 段 + ffmpeg 截 vocals/audio → shared/ref_voice.{wav,txt}）→ `TtsEngine::with_task_reference` 注入 ZipVoice（任务级覆盖全局参考，失败回退）。
- **UI 自适应**：HomePage 步骤条（9 步一行 → `flex-wrap` + 行线限宽）、`.form-grid` 窄屏单列、`.action-summary` 收缩 ellipsis、`.queue-mini` 限宽——「生成字幕/合成视频脱框」「选中文后跳行」根因修复。仍有拿不准的布局项请截图确认。

### 阶段六：文档体系重构（2026-09-03 会话，verifier t32636-1 胜者方案 B）

- **分层模块文档体系**（目标：AI 仅凭架构图 + README 可复现代码逻辑）：
  - `docs/ARCHITECTURE.md`（新，全局权威）：分层总图 + 依赖规则 + 模块索引 + 横切约束；
  - 模块级 5 份：`src-tauri/src/{pipeline,adapters,engines,application}/ARCHITECTURE.md` + `src/lib/ARCHITECTURE.md`（含 views）——各自 DAG 图 / port 矩阵 / 装配流程 / 队列状态机 / 改动指引；
  - README 加端到端数据流总图（mermaid）+ 文档导航表，测试基线 194→204 修正；
  - 旧 `BACKEND_ARCHITECTURE.md`（重构前设计契约，约一半过时）归档至 `docs/archive/BACKEND_ARCHITECTURE_v2_refactor_plan.md`，全部引用点已改。
- **架构审查结论（实测）**：分层清晰、依赖方向零违规（legacy 零引用、无向上依赖）、最大文件 825 行；无需改代码，本轮纯文档。

### 阶段七：测试矩阵 + VAD 真 bug 修复（2026-09-04 会话，verifier t32636-3 胜者 C）

- **真机场景测试矩阵**（`application/scenario_tests.rs`，9 场景薄壳 + 共享 run_scenario + 声明式断言）：真实视频 `E:\projects\pyvideotrans-3.98\10.mp4`（VT_TEST_VIDEO 可覆盖）跑完整流水线，翻译走本地 mock（TcpListener 假 OpenAI 服务，免费稳定）。场景：①基础 supertonic ②zipvoice 中文 ③多目标 ④粤语方言 ⑤原声克隆（自备 5s 合成素材，10.mp4 语音仅 0.18s 不合格）⑥背景分离 ⑦双音轨（ffprobe 验证轨数=2）⑧断点复用（二次运行 mtime 不变）⑨字幕编辑触发下游重跑。单场景：`cargo test --features inference -- --ignored scenario_dual_track`。
- **VAD 真 bug 修复**（场景矩阵实证）：sherpa-onnx `accept_waveform` 一次性喂超长音频内部状态机异常截段（12s 语音只剩尾部 0.3s "Yeah."），threshold/window/模型版本均无关 → 改官方 example 模式：按 512 分块喂 + 逐块取段 + flush 后 drain（`engines/stt/sensevoice.rs`）。修复后 12s 合成语音完整切出两段，识别正常。
- **VAD 模型替换**：旧 643KB 文件实为 silero **v5**（官方 `silero_vad.onnx` 现指向 v5），已从官方 release 下载 **v4**（1.8MB，`silero_vad_v4.onnx`，用用户浏览器下载）替换为 `silero_vad.onnx`；v5 备份 `silero_vad_v5.onnx.bak`。v4/v5 在分块喂修复后行为一致。
- **e2e 体系修复**：e2e.rs / e2e.mjs / pipeline_service.rs 旧默认模型路径更新到 test2voices_backup；e2e.mjs 预检新增 10.mp4 与 UVR 模型项；`vad_diagnostic` 诊断用例保留为 VAD 行为探针（window=256/1024 对 v5 会 ORT abort，勿试）。
- **素材事实**：10.mp4 前 9.67s 为音乐/背景（Silero threshold=0.2 也判非语音），仅尾部 0.31s 人声 "Yeah."——此前「开头幻觉字幕」修复行为正确。
- **门禁基线更新**：cargo test 默认 **204/17**（ignored 17 = 原 16 + vad_diagnostic）；真机全量 `-- --ignored` **17/17 通过（~4.5 分钟）**；npm test 66。

---

## 3. 关键边界（用户两次叫停，务必遵守）

⛔ **不做结构性文件拆分**。用户已两次取消「拆大文件」动作：
- **S7（infra 拆分 `task_store/artifact_store`）**：取消——两文件本就达 ≤800 行目标。
- **helper.rs 拆 4 子模块**：取消。

✅ 允许做的是：**优化、死代码删除、重复提取（就地/收敛）、零风险微性能**。
❌ 不做的是：为「拆小」而做的文件/目录重组（churn）。
后续会话**不要再尝试 S7 或 helper.rs 拆分**。

其他既有约束：Windows 内存紧（`.cargo/config.toml jobs=2` 勿删、勿并行多 cargo 编译）；RAII 租约保持在 executor 主方法作用域；改主流程/多语言/恢复策略须同步 `docs/FUNCTION_CHECKLIST.md`；最终回复用中文。

---

## 4. 当前架构速查（代码地图）

```
src-tauri/src/
├── lib.rs                 # composition root（168 行：mod 声明 + run()；31 命令注册）
├── state.rs               # RunningTask + TASKS 全局任务表
├── types.rs               # AppConfig（含 zipvoice_* 字段）/ Segment / ProgressEvent
├── commands/              # 全部 31 个 Tauri 命令（task/config/api_test/tasks/media_tools/runtime/realtime_stt/diarization）
├── engines/               # 裸引擎（stt/{sensevoice,whisper_native,openai_api,whisper_cli}、translate/deepseek、tts/supertonic/{mod,helper,assets}）
├── adapters/              # ports 实现（stt/tts/translate/separation/media；tts/ 含 cosyvoice3/supertonic/zipvoice）
├── legacy/                # 【冻结】旧单目标流程（command=start_task + process + ffmpeg），仅紧急回滚
├── pipeline/              # graph / registry / runner/{mod,types,records,executor,tests}
├── application/           # pipeline_service / task_service / checkpoint / subtitle_*
├── domain/ ports/         # 纯类型 / 纯 trait
├── infra/                 # task_store / artifact_store / api_client / diskcheck
├── tts_dub.rs             # TTS 三引擎共用：时间轴组装 + 对齐（无门控）
├── scheduler.rs           # 资源调度（成本声明 + 准入 + RAII 租约）
├── memcheck.rs            # commit 内存预审（STT 开工前选 30s/15s 窗口）
├── audio_io.rs / audio_align.rs / segments.rs / subtitle*.rs / text_align.rs / logger.rs / e2e.rs
```

**模型路径**（2026-09 跨机迁移后实测，E 盘新位置）：
- SenseVoice：`E:\projects\test2voices_backup\sense-voice-int8`（含 silero_vad.onnx）
- ZipVoice：`E:\projects\test2voices_backup\sherpa-onnx-zipvoice-distill-int8-zh-en-emilia\`（含 vocos_24khz.onnx + test_wavs/）
- Supertonic：`E:\projects\pyvideotrans-3.98\Supertone\supertonic-3`（未移动，中文仍缺 `*_zh.onnx` 扩展→中文目标用 ZipVoice）
- Whisper large-v3-turbo：**跨机迁移后 E 盘未找到**（默认值暂留旧路径，选 whisper_native 时 preflight 会拦截；需重新下载或用户提供位置）
- 以上已同步 `types.rs` default_* 与 `engines.ts` DEFAULT_CONFIG（2026-09-03）

**回滚网**：`videotrans-tauri/.refactor-backup/{src-tauri-src, src}`（项目无 git，唯一备份，勿动）。

---

## 5. 剩余事项 / follow-up

- **用户启动后必做**：① 设置页填 DeepSeek API Key（翻译外部依赖）；② 中文目标 TTS 选 ZipVoice（supertonic 中文缺 `*_zh.onnx` 扩展，参考音频已默认填好）。
- **Supertonic-ZH 扩展**（若要用中文 supertonic）：`*_zh.onnx` 三件 + `unicode_indexer_zh.json` 放入 `onnx/`。
- **明确未做**（评审否决/无实证）：前端 happy-dom 冒烟；scheduler 退避改超时报错；git 初始化提交。
- **未接线能力**（实现完备、无调用者，已登记 `FUNCTION_CHECKLIST.md §3`）：ArtifactStore 安全路径 API、`ApiExecution::Local` 本地 LLM、`PipelineRunner::run_targets` 并行、`TaskStore` 自愈/巡检。接入前先复查与现写入/调度路径兼容性。

---

## 6. 操作手册（验证命令）

```bash
cd videotrans-tauri/src-tauri && cargo test          # 后端（须 190 过 / 7 ignored）
cd videotrans-tauri/src-tauri && cargo check --no-default-features   # 无推理编译
cd videotrans-tauri && npm run test                  # 前端（须 27 过）
cd videotrans-tauri && npx vue-tsc --noEmit          # 前端类型
cd videotrans-tauri && npx vite build                # 前端构建
# 真机 e2e（需本地模型）：
cd videotrans-tauri/src-tauri && cargo test --features inference -- --ignored --nocapture
cd videotrans-tauri && npm run tauri dev             # 开发运行（含推理）

# target/debug 持久瘦身（debug=1 已配置后，全量重建一次；~20-40 分钟）：
cd videotrans-tauri/src-tauri && cargo clean && cargo test
```

**日志/排查**：`%LOCALAPPDATA%\videotrans\logs\`（`videotrans-YYYYMMDD.log`、`failures.jsonl`、`crash.log`——闪退先看 crash.log）。

**参考**：当前架构 `docs/ARCHITECTURE.md`（全局）+ 模块级 ARCHITECTURE（pipeline/adapters/engines/application/src-lib）；需求跟踪 `docs/FUNCTION_CHECKLIST.md`；项目规则 `pyvideotrans-3.98/AGENTS.md`（多方案必比、教学模式、日志排查）。

**Suggested skills**：`diagnose`（测试变红时：复现→最小化→假设→定位→修复）、`tdd`（新功能先 contract 测试）。
