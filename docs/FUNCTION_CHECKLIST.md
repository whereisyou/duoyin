# VideoTrans Tauri 功能清单与需求跟踪

> 活文档。任何涉及主流程、历史任务、多语言/多方言、背景音分离、视频合成、任务恢复的设计或实现，必须先对照并更新本文档。  
> 最近更新：2026-08-20。  
> 分区：基础需求 / 次要需求 / 待规划需求 / 待对齐需求。  
> 原则：先需求对齐，再数据模型，再 fake pipeline 测试，最后实现真实功能。

---

## 0. 观察依据（源码线索）

- 原 Python README：主流程是 ASR → 翻译 → TTS → 视频合成；工具集包含人声分离、视频/字幕合并、音画对齐、文稿匹配。
- `videotrans/task/_base.py`：基础任务类拆成 `prepare / recogn / diariz / trans / dubbing / align / assembling`。
- `videotrans/task/separate_worker.py`：背景音分离产物为 `vocal-*.wav` 与 `instrument-*.wav`，并会先转 44.1kHz WAV。
- `videotrans/component/clip_video.py`：工具支持默认、仅视频、仅音频、音视频分离。
- `videotrans/component/onlyone_set_editdubb.py`：支持配音结果校对与重新配音。
- `videotrans/component/onlyone_set_role.py`：支持字幕行分配角色和试听配音。
- `videotrans/component/realtime_stt.py`：原软件有实时语音识别工具。
- `videotrans/component/set_ass.py`：原软件支持 ASS 样式设置/预览。
- `videotrans/tts/_cosyvoice.py`：已存在 CosyVoice3 方言/风格控制接入线索：`cosyvoice_instruct_text` 走“自然语言控制”(inference_instruct2)，例如“请用河南话表达。”。
- `videotrans/tts/_doubao.py`：存在方言枚举参考：东北、粤语、上海、西安、成都、台湾、广西。
- `text2voices/CosyVoice/cosyvoice/utils/common.py`：CosyVoice3 instruct 列表包含：广东话、东北话、甘肃话、贵州话、河南话、湖北话、湖南话、江西话、闽南话、宁夏话、山西话、陕西话、山东话、上海话、四川话、天津话、云南话。
- `docs/speaker_consistency/SPEAKER_DIARIZATION.md`：CAM++ 位于说话人分离/说话人日志场景，不属于背景音源分离。

---

## 0.1 已确认决策

- 背景音分离手动开启；开启后与 STT 并行。
- 背景音分离用于后续合成保留背景音，不作为 STT 前置依赖。
- 背景音分离失败时，最终视频可退化为“无背景音，只用 dub.wav”，该退化策略属于背景音分离高级设置。
- `vocals.wav` 与 `bgm.wav` 都必须保存。
- 背景音分离降噪/音量归一化属于二级高级设置，产物命名要区分 raw/normalized。
- 背景音分离优先资源占用低、成功率高的模型；需匹配用途：CAM++ 是说话人识别/日志模型，不是人声/背景音源分离模型；基础版分离候选优先 UVR-MDX-NET ONNX。
- 背景音分离采样率按模型要求，降低用户配置复杂度。
- 中文默认代表中文普通话；若用户开启方言设置，则只按用户选择的方言版本输出。
- 方言入口位于目标语言右侧齿轮；不支持方言的语言置灰或不显示入口。
- 常用组合预设仅在用户开启方言设置后出现，不能增加默认主界面复杂度。
- 父任务详情页需要展示共享产物状态；父级共享产物空间单独显示。
- 每个目标版本默认折叠显示总空间，展开显示字幕/音频/视频等明细。
- 子任务失败后父任务显示“部分失败”。
- “重跑所有目标语言”属于后续迭代；未来采用单选阶段模式。
- 用户手动编辑/替换文件后的“重新校验产物”按钮属于后续迭代；当前先自动跑通。
- 保留原音轨 + 添加新配音轨默认不开启；默认替换音轨。
- 拉伸/变速默认范围 0.85x~1.25x。
- 硬字幕样式属于后续优化，现在不做。
- 所有新增设置必须持久化，关闭应用重启后可恢复。

---

## 1. 基础需求（当前主流程必须做）

### 1.1 主流程目标

基础版要闭环：

```text
选择视频
  → 媒体探测
  → 提取音频
  ├─ STT 识别字幕
  └─ 背景音/人声分离（用户开启时，与 STT 并行）
  → 翻译到一个或多个目标语言/方言
  → TTS 生成各目标版本配音音频
  → 混音（有 bgm 时 bgm + dub，否则 dub）
  → 可选合成最终视频
  → 输出字幕/音频/视频产物
  → 支持历史任务恢复与继续生成
```

### 1.2 输入、任务与队列

- [x] 批量选择/拖入视频文件。
- [x] 内存队列：排队/运行/完成/失败/取消。
- [x] 多维调度支持 CPU/GPU/进程槽/RAM RAII lease、任务卷磁盘预审、RemoteApi token bucket 与 LocalApi 双准入。
- [x] 主页面底部任务摘要显示运行/排队/完成/错误与当前任务进度。
- [x] 父子任务模型已接入多目标 command、持久化 TaskStore 与任务页父子关系展示：
  - 父任务：源视频、媒体探测、音频提取、可选背景分离、STT。
  - 子任务：每个目标语言/方言的翻译、TTS、字幕、混音、可选视频。
  - 队列中拆成子任务，但必须展示清晰父子关系。
- [x] 子任务失败后父任务聚合 `PartiallyFailed`，任务页展示失败子版本与错误。
- [x] 新多目标 Pipeline 中 STT 是父级共享前置，失败阻塞全部子任务。
- [x] `run_targets` 独立收集结果，某目标失败不阻塞其他版本。
- [x] 任务页支持单独取消目标版本，独立 CancelToken 不影响父任务与其他版本。
- [x] 父任务 STT 完成后所有子版本并发入队，由 API token bucket 与本地资源 lease 决定实际并发。
- [x] Runner StageObserver 发共享/目标阶段事件，前端仅以 `parent + step=done` 作为父任务终态，避免中间 stage 完成导致任务假完成；任务页展示共享阶段、子任务汇总、父状态与空间：
  - 共享阶段：媒体探测/提取音频/背景分离/STT 各自状态。
  - 子任务汇总：总数、运行中、已完成、失败。
  - 父任务整体状态：运行中 / 部分失败 / 已完成。
  - 底部和任务卡按真实 DAG 工作单元汇总进度（共享阶段一次 + 各目标版本阶段），详情页展示分段状态。

### 1.3 媒体探测与稳定目录

- [x] 新 Pipeline 真实 ffprobe 覆盖完整媒体信息，任务详情可按需展开 MediaInfo artifact。
- [x] 提取 16kHz mono WAV（旧流程与新 MediaTool adapter 均有真实 FFmpeg 测试）。
- [x] 前端统一调用多目标新 Pipeline，任务稳定写入 `%LOCALAPPDATA%/videotrans/tasks/{task_id}/targets/{variant_id}` 并可启动恢复。
- [x] 后端 SourceFingerprint 已记录 size、mtime、可选 hash 与 hash 算法版本。

### 1.4 背景音/人声分离（基础重要功能）

- [x] 用户在高级设置手动开启背景音分离。
- [x] 分离直接消费源视频高质量音轨，仅用于后续 BGM 混音，不作为 STT 输入。
- [x] 分离与“提取 16k 音频→STT”两条父级分支并行。
- [x] sherpa-onnx UVR-MDX C API adapter、30s 流式窗口、1s crossfade、双产物事务已实现并通过真实模型 e2e。
- [x] 分离产物默认保存：
  - `vocals.wav`
  - `bgm.wav`
- [x] 分离产物 retention 独立于 final，不输出视频也保留。
- [x] 分离失败按高级设置退化为 no_bgm，mix/final 仅使用 dub.wav。
- [x] 失败退化开关位于二级高级设置。
- [x] adapter 自动转 44.1kHz stereo，采样率不暴露主界面。
- [x] 基础模型采用原 Python 同款 `UVR-MDX-NET-Inst_HQ_4.onnx`，支持一键下载/本地选择。
- [x] CAM++ 明确不用于背景音分离。
- [later] 阿里相关可关注：MossFormer2 SE（语音增强/降噪，不是 BGM 分离）、SAM-Audio（文本引导源分离，资源较重，后续评估）。
- [x] 降噪/音量归一化位于二级菜单，默认关闭。
- [x] FFmpeg afftdn/loudnorm 后处理产物命名区分：
  - `vocals.raw.wav`
  - `vocals.normalized.wav`
  - `bgm.raw.wav`
  - `bgm.normalized.wav`

### 1.5 STT 字幕识别

- [x] SenseVoice 本地轻量 STT。
- [x] Whisper candle 本地高质量备选；支持源语言 `auto` 首窗语言识别并在后续窗口复用语言 token。
- [x] OpenAI Whisper API。
- [x] whisper.cpp CLI 备选。
- [x] 所有 STT adapter 在 port 边界统一清洗字幕段：过滤 NaN、倒置、零长度、空文本并重编号；单个坏段不再拖垮整次识别，全无有效段时给出可行动错误。
- [x] STT 结果作为父级共享 `shared/segments.json` 持久化。
- [x] 字幕编辑页可保存父级原文，自动标记所有目标翻译及下游失效。

### 1.6 多语言/多方言目标版本

- [x] 多语言与多方言使用独立 TargetVariant 数据模型。
- [x] 方言分别携带 `translate_style` 与 `tts_accent`。
- [x] 基础版不提供每方言独立音色。
- [x] 多目标 Pipeline 只执行一次父级 STT，所有版本共享 segments。
- [x] 工作台目标语言已改为多选列表。
- [x] 主界面只保留原语言和目标语言，复杂项进入 modal。
- [x] 原语言支持“自动识别”。
- [x] 方言设置入口位于目标语言右侧齿轮。
- [x] 未选择中文时方言齿轮禁用。
- [x] 后端内置 descriptor + 应用同级 `config/dialects.json` 覆盖扩展。
- [x] 方言多选位于二级 modal，不增加主界面复杂度。
- [x] 中文默认映射中文普通话。
- [x] 保存方言设置后只生成勾选的中文版本。
- [x] 普通话是方言多选项之一。
- [x] 粤语属于中文目标分支并可与其他方言多选。
- [x] 中文方言内置集合覆盖 CosyVoice3 instruct 全部已确认方言 + 普通话：
  - 普通话（默认）
  - 广东话/粤语
  - 东北话
  - 甘肃话
  - 贵州话
  - 河南话
  - 湖北话
  - 湖南话
  - 江西话
  - 闽南话
  - 宁夏话
  - 山西话
  - 陕西话
  - 山东话
  - 上海话
  - 四川话
  - 天津话
  - 云南话
- [x] 方言扩展配置已实现内置数据 + 应用同级 `config/dialects.json`。
- [x] 方言 modal 允许只选粤语而不生成普通话。
- [later] 常用组合预设当前不做；已归入待规划需求。
- [x] 工作台显示将生成的目标版本预览名：
  - `中文（普通话）`
  - `中文（粤语）`
  - `英语`

### 1.7 翻译

- [x] OpenAI-compatible 翻译接口。
- [x] API 连通测试。
- [x] 新 ApiClient 统一 token bucket、Retry-After、429/5xx/网络重试、jitter、deadline 与脱敏日志。
- [x] JSON 返回兼容：数组、`translations` 对象、单对象。
- [x] 多目标翻译共享 AppConfig API limiter。
- [x] 每个目标版本保存：
  - `targets/{variant_id}/translated.json`
  - `targets/{variant_id}/translated.srt`
- [x] 方言 translate_style 进入翻译 prompt。
- [x] 字幕编辑页保存某版本译文后，仅标记该版本 TTS/SRT/mix/final 失效。
- [x] 任务页允许导入某版本外部 SRT，同时生成 translated.json。
- [x] 外部 SRT 标记 translate external_override，重试时跳过翻译直接进入 TTS。

### 1.8 TTS 与目标版本音频

- [x] Supertonic 本地 TTS；任务启动前校验目标语言与模型文件，中文缺少 Supertonic-ZH 时列出具体缺失文件并建议切换 CosyVoice3，不再先跑昂贵 STT/翻译后失败。
- [x] CosyVoice3 FastAPI `/inference_instruct2` 基础接入，支持参考音频音色 + 方言 instruct，裸 PCM 按可配置采样率写 WAV。
- [ ] CosyVoice3 达到 pyVideoTrans 级接入能力：显式协议选择（FastAPI instruct2 / zero-shot、Gradio `/generate_audio`、旧版 `clone_eq/clone_mul`）、参考文本、参考音频不足 3 秒自动补静音、speed、PCM/WAV 响应识别、可重试错误与配置错误分级；完成真实服务 contract 测试后才能勾选。
- [x] ZipVoice-Distill INT8 本地零样本 TTS：普通话基础版使用全局参考音频 + 对应参考文本，基于 sherpa-onnx Rust API、模型前置校验、重资源独占、逐段生成并流式写 `dub.wav`、取消与真实 TTS→STT e2e（真机 24kHz 验证通过）；**任务级原声克隆**（2026-08-25，verifier t27348-1）：高级设置 `tts_use_video_prompt` 开启后由 TTS stage 自动从 shared/segments 挑最长语音段（3~20s）经 ffmpeg 截取为 `shared/ref_voice.wav` + 原文 txt，经 `TtsEngine::with_task_reference` 注入 ZipVoice，失败回退全局参考。
- [x] 新 Pipeline TTS stage 为每个版本生成并保留 `targets/{variant_id}/dub.wav`。
- [x] final_video stage 只读取 `dub/mixed`，不会删除 `dub.wav`，便于恢复复用。
- [x] 后端多版本目录已按 `targets/{variant_id}/dub.wav` 拆分并有双目标集成测试。
- [x] 方言 `tts_accent` 传入 CosyVoice3 instruct2 控制口音。
- [x] 基础版使用全局 TTS 引擎；每语言独立引擎/音色保留待规划。
- [later] 配音结果校对/重新配音 UI 属于次要增强；后端 DAG 已支持仅失效对应版本下游。

### 1.9 混音与最终视频合成

- [x] FFmpeg mix stage 输出 `targets/{variant_id}/mixed.wav`。
- [x] 背景分离有效时，真实音频测试覆盖 `bgm.wav + dub.wav → mixed.wav`。
- [x] 背景分离失败/未开启时，mix stage 仅使用 `dub.wav`。
- [x] 前端统一走新 Pipeline；全局开关控制每个目标版本是否生成 `final.mp4`，产物存在后才完成。
- [later] 每语言单独勾选 final 已归入待规划；基础版使用全局开关。
- [x] final_video stage 输出到 `targets/{variant_id}/{源文件名}.{variant_id}.mp4`（可选 `final.mp4`），双目标集成测试通过。
- [x] 输出视频、原音轨、字幕和变速范围位于工作台高级设置 modal。
- [x] 基础合成选项已接高级设置；硬字幕显示为禁用的“后续支持”占位：
  - 重新合成视频并使用新配音音轨，不烧字幕（默认）
  - 重新合成视频并使用新配音音轨 + 外挂 SRT
  - 重新合成视频并使用新配音音轨 + 硬字幕（功能占位，样式后续优化）
  - 保留原音轨 + 添加新配音轨（默认不开启）
- [x] Supertonic/CosyVoice3 单段过长时使用 FFmpeg rubberband 保持音高变速，超限记录 warning 并采用保守上限。
- [x] 默认范围持久化为 0.85x~1.25x；短段采用静音填补，长段最大 1.25x。
- [x] 变速范围位于视频高级设置。
- [x] 命名规则提供默认 `源文件名.版本.mp4` 和 `final.mp4` 两种方案。
  - 默认建议：`{原文件名}.{variant_id}.mp4`

### 1.10 历史任务与产物复用

- [x] `tasks/index.json` 后端原子缓存与损坏重建。
- [x] `task.json` 后端 revision/`.tmp`/`.bak` 持久化协议。
- [x] `manifest.json` 后端 revision 一致性提交与备份恢复；Runner 在每个状态边界自动 checkpoint。
- [x] 阶段级产物校验支持路径安全、存在性、size/mtime、流式 SHA-256、状态刷新与 DAG 失效。
- [x] Runner 每个状态边界通过 TaskStore checkpoint 原子持久化；启动恢复将 Running stage 标记 `Interrupted`，父子任务恢复 Pending。
- [x] 新主入口按 Artifact retention 默认保留 audio/segments/translated/dub/mixed/SRT/final。
- [x] **SenseVoice 静音幻觉门控**（2026-08-25）：引擎接入 Silero VAD（模型目录放 `silero_vad.onnx` 即启用，缺失时行为不变）——先切语音段再识别，消除开头静音/背景音乐被识别成语音的幻觉字幕。
- [x] 必要文件存在且 size/mtime/SHA-256 有效时复用，缺失或失效按 DAG 重跑。
- [x] 外部编辑 segments/translated/SRT 后接受新 SHA-256，保留编辑文件并仅失效下游。
- [x] 启动 reconcile 检测删除的必要文件并按 DAG 仅重跑对应链路；任务页重试保留原 `task_id`，不会新建目录丢失可复用产物。
- [later] 手动重新校验按钮已归入待规划；当前启动自动 reconcile。
- [x] 历史任务页单独显示父级共享产物空间。
- [x] 每个版本显示空间总量和输出目录，任务详情按钮按文件列出类型/路径/大小/状态。
- [x] 删除/清空任务：后端 `delete_persistent_task` 删除任务目录并同步剔除 `index.json` 条目（原子重写，幂等），运行中任务拒绝删除；前端删除/清空先落盘成功再更新本地列表，重启不复活（2026-08-25 修复，原实现仅内存 splice）。
- [x] 必要文件落盘约定：
  - 父级：`audio.wav`、`vocals.wav`、`bgm.wav`、`stt/segments.json`
  - 子级：`translated.json`、`translated.srt`、`dub.wav`、`mixed.wav`、`final.mp4`（如果开启）

### 1.11 设置持久化

- [x] 基础配置持久化。
- [x] 多语言目标、方言 descriptor 与任务配置快照持久化，重启后历史任务恢复。
- [x] 二级高级设置写入应用同级 `config.json`，关闭应用重启后恢复；兼容读取旧 `%APPDATA%/videotrans/config.json`。
- [x] 开箱默认模型路径：后端 `AppConfig::default()` 携带 4 个本机模型目录 + ZipVoice 参考音频，加载时 `normalize_defaults()` 对空字符串字段回填（前端 DEFAULT_CONFIG 同步，双端契约测试防漂移）——修复旧配置空字段覆盖导致 TTS 误报「需要配置」（2026-08-25）。

---

## 2. 次要需求（常用增强，但不阻塞主流程）

- [x] 媒体工具页支持视频时间段裁剪。
- [x] 媒体工具页支持视频流/音频流分离与重新合并。
- [x] 字幕与文稿按行或原文字符权重匹配，输出新 SRT。
- [x] MediaRecorder 5 秒分片 + 当前 STT 引擎实时识别，支持自动语言。
- [x] ASS 字体/字号/颜色样式预览与导出。
- [x] UVR 一键下载、各模型完整性/大小状态管理与本地路径配置。
- [x] 设置页检测 NVIDIA GPU 名称和显存；本地模型仍由 Scheduler 选择 CPU/GPU 资源画像。
- [x] 远程翻译 ApiClient 支持显式 HTTP/HTTPS 代理；localhost 模型保持直连。
- [x] **翻译容错**（2026-08-25）：translate stage 自动重试 3 次（退避 0.8s/1.6s）吸收偶发截断；仍 IncompleteResult 时以原文回填译文并标记 degraded（manifest 可见、可重试），不再因漏翻一段整体失败。
- [x] 任务页支持试听 dub、编辑译文、导入 SRT，并按现有产物重新生成，仅重跑失效下游。
- [x] sherpa-onnx pyannote segmentation + speaker embedding 说话人分离，生成 `shared/speaker.json`。

---

## 3. 待规划需求（明确现在不做）

- [later] 不同目标语言使用不同翻译模型/API。
- [later] 每个目标语言/方言独立 TTS 引擎。
- [later] 每个语言版本独立音色选择。
- [later] 多角色配音下，每个语言版本单独角色→音色映射。
- [later] 每个语言单独勾选是否输出最终视频。
- [later] “重跑所有目标语言”按钮与阶段选择。
  - 未来设计：单选模式选择从哪个阶段重跑；只要阶段之前文件存在且有效，就从该阶段重跑。
- [later] 用户手动编辑/替换文件后的“重新校验产物”按钮。
- [later] 更高质量分离模型：Demucs / BS-Roformer / MDX23C 等。
- [later] 硬字幕样式/ASS 样式编辑。
- [later] 更丰富翻译渠道：Google/Microsoft/Baidu/Tencent/Azure/DeepL/Gemini/Ollama/M2M100 等。
- [later] 更丰富 TTS：Edge/Azure/OpenAI/QwenTTS/F5/GPT-SoVITS/ChatTTS/ElevenLabs/Minimax 等。
- [later] ZipVoice 中文方言能力；基础版仅承诺普通话，方言继续使用 CosyVoice3 instruct。
- [later] **重构遗留的未接线能力**（2026-08 dead_code 清理时登记，属实现完备但无调用者的预留面；接入前先复查其与现写入/调度路径的兼容性）：
  - `ArtifactStore` 安全路径 API（`scoped_path`/`temp_path_for`/`staging_layout`/`staging_root`/`is_orphan_candidate`/`prepare_target`/重解析点校验 `artifact_store.rs`）——写入路径重构后集中在 executor `commit_file`，该安全纵深未接线；接入或删除前需评估路径穿越防护归属。
  - 本地 API 执行（`ApiExecution::Local` + `scheduler::admit_local_api` + `OpenAiCompatibleTranslator::new_local`）——本地 LLM 服务（Ollama 等）复用接口，实现已在 `api_client.execute`。
  - `PipelineRunner::run_targets` 并行驱动多目标——当前生产走 `run_parent` + `run_targets_with_tokens`（串行 + 每目标取消令牌）；若未来恢复并行调度可直接接回。
  - `TaskStore::cleanup_orphan_temps` / `rebuild_index`——启动自愈/巡检未接线。

---

## 4. 待对齐需求（禁止靠猜实现）

当前没有阻塞架构设计的待对齐需求。

### 4.1 已转为技术选型/实现约束

- 背景音分离基础版模型：使用与原 Python 项目一致的 `UVR-MDX-NET-Inst_HQ_4.onnx`。
  - 理由：ONNX 原生、模型体积约 59MB、资源占用低，且已通过真实模型 e2e。
  - 设计约束：模型作为可替换 descriptor，不写死到流水线核心；后续可替换其他低资源模型。
- 外挂 SRT：基础版先输出 `.srt` 文件；mux 为 mp4 软字幕轨属于待规划需求。
- 未来硬字幕占位：在视频合成高级设置中以“后续支持”提示展示，不进入主界面。

---

## 5. 建议实施顺序

### 5.0 测试矩阵（2026-09-04）

- 单元/集成：cargo test **204/17** + npm test **66**（variants/srt/ass 序列化与方言展开逻辑直测）。
- 真机场景矩阵（`application/scenario_tests.rs`，10.mp4 + mock 翻译）：①supertonic 基础 ②zipvoice 中文 ③多目标 ④粤语方言 ⑤原声克隆（合成素材）⑥背景分离 ⑦双音轨（ffprobe 轨数=2）⑧断点复用（二次运行 mtime 不变）⑨字幕编辑触发下游重跑；`npm run e2e` 全量 17/17（~4.5 分钟）。
- STT VAD 真 bug 已修（2026-09-04）：sherpa-onnx accept_waveform 一次性喂超长音频异常截段 → 改分块喂（512/块）+逐块取段；VAD 模型换官方 v4（1.8MB）。
- 已知素材事实：10.mp4 前 9.67s 为音乐（无人声），仅尾部 0.31s "Yeah."——选语音丰富的视频测克隆/识别体验更佳。

需求层已对齐，当前基础/次要需求已接入真实多目标入口；**旧单目标流程已冻结隔离至 `src-tauri/src/legacy/`（`command.rs` 的 `start_task` + `process.rs` + `ffmpeg.rs`），仅作紧急回滚，不再作为前端入口，禁止新代码依赖它**。

- 冻结区移除标准（全部满足才可删）：见 `docs/archive/BACKEND_ARCHITECTURE_v2_refactor_plan.md` §6 —— 连续 2 个发布版本无生产回滚使用、新 pipeline 具备等价失败恢复能力、新路径支持 legacy 全部引擎组合、经显式需求评审并更新本文档、紧急 runbook 不再引用 `start_task`。

1. 继续补全 Python 原软件基础能力观察（作为架构参考，不照搬 UI/代码）：
   - 视频合成
   - 背景音分离
   - 音画对齐
   - 多角色配音
   - 任务恢复/中间产物
2. 设计数据模型：
   - `TaskManifest`
   - `ParentTask`
   - `ChildTask`
   - `TargetVariant`
   - `Artifact`
   - `StageDependencyHash`
3. 写 fake pipeline 测试：
   - 首次全流程
   - 重启恢复
   - 分离与 STT 并行
   - 某子任务翻译失败，其他继续
   - 编辑父级 STT 后下游全部失效
   - 编辑单语言译文后仅该语言下游失效
4. UI 与真实流水线已接入；后续以真实 App 人工验收和入口回归测试为主。

### 5.1 当前实施进度（2026-08-19）

- [x] Domain：Parent/Child/Variant/Artifact/Manifest/配置快照。
- [x] ArtifactStore：路径逃逸防护、同卷 staging、文件状态检查。
- [x] TaskStore：revision 一致性、`.tmp/.bak` 恢复、索引重建。
- [x] Fake Pipeline：DAG、single-flight、取消、降级、父子隔离、拓扑失效、磁盘重启复用。
- [x] MediaTool port + 可取消 FFmpeg/ffprobe adapter + 真实样片 contract test。
- [x] STT ports/stages 与现有 SenseVoice/Whisper/API/CLI 引擎接入新 Pipeline。
- [x] 翻译/TTS ports、OpenAI-compatible、Supertonic、CosyVoice3 方言接入。
- [x] sherpa UVR-MDX 背景分离 adapter、一键下载、流式窗口与真实模型 e2e。
- [x] SRT/mix/final stages、多目标 commands/UI、历史恢复与父子任务进度接入。
- [x] 真实离线 application e2e：Supertonic → 视频 → SenseVoice → mock OpenAI 翻译 → 两目标 TTS/mix/SRT/final → TaskStore，全部通过。
- [x] 真实 UVR模型 e2e与33秒 Supertonic→SenseVoice 跨30秒窗口 e2e通过。
- [x] Whisper candle跨窗口 e2e在本机commit仅1.7GB时按内存预审安全跳过，未发生OOM/崩溃。
- [x] 真实入口回归：父级/子级中间阶段完成不会被误判为任务终态；历史任务重试沿用原 task_id；历史共享阶段从 manifest 恢复真实状态。
- [x] 验证矩阵：默认 inference 后端 182 passed / 6 ignored；无 inference 后端 158 passed；前端 27 passed；vue-tsc 与 vite build 通过（2026-08-19）。
- [ ] 当前迭代：ZipVoice-Distill INT8 普通话零样本接入与 CosyVoice3 pyVideoTrans 级协议兼容补全（2026-08-20 开始）。
- [x] 用户真实失败音频回归：Whisper 显式 `en` 不再因零宽段整体失败；`auto` 首窗正确识别 `zh`，字幕尾段钳制在真实音频时长内。
- [x] 可观测性回归：stage/任务失败同时写每日运行日志与 `failures.jsonl`；应用加载历史失败任务时从 Manifest 补录 task_id、stage、error 与任务目录。
