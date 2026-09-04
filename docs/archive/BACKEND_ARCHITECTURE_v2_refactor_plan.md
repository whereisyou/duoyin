# VideoTrans Tauri 后端代码架构设计（v2）【已归档 2026-09-03】

> ⚠ **本文是历史设计契约，不代表当前状态。** 重构已全部落地，当前架构唯一权威文档是 [`docs/ARCHITECTURE.md`](../ARCHITECTURE.md) + 各模块文件夹内 `ARCHITECTURE.md`。
> 保留本文仅作历史决策参考（切片计划、会诊记录、legacy 移除标准 §6 仍有效）。

> 本文是当时重构的目标架构与实施契约。  
> 决策依据：4 模型会诊（claude/deepseek/glm/gpt55 一致选「legacy 隔离、不删」）+ verifier 锦标赛（C1 胜出，均分 0.518 > C3 0.498 > C2 0.484）。  
> 基线（重构前实测）：后端 18,421 行 Rust，182 passed / 6 ignored；前端 5,271 行，27 passed。  
> 最高约束：**功能零回归**。29 个 Tauri 命令 IPC 名不变、测试语义不变、legacy 回滚路径不悄悄失效。

---

## 1. 现状诊断（实测，非推测）

| 问题 | 证据 |
|---|---|
| 巨型文件 | `lib.rs` 915 行（bootstrap + 16 个内联 `#[tauri::command]` + `RunningTask` + 全局 `TASKS` 静态）；`pipeline/runner.rs` 1758 行（公共类型 + `PipelineRunner` 核心 ~700 行 + 记录辅助 + ~790 行测试）；`supertonic_helper.rs` 890 行；`infra/task_store.rs` 769 行；`infra/artifact_store.rs` 580 行 |
| 裸引擎平铺 crate 根 | `stt_sensevoice.rs` 294、`stt_whisper_native.rs` 545（candle，活的）、`api_openai_stt.rs` 121、`api_whisper_local.rs` 66、`api_deepseek.rs` 227、`tts_supertonic.rs` 454、`supertonic_helper.rs` 890 —— 同时被 legacy 旧流程和 ports/adapters 新体系使用 |
| ports/adapters 语义被稀释 | `adapters/stt/legacy.rs`（ConfiguredSttEngine）、`adapters/tts/supertonic.rs` 是包在裸引擎外的 port 实现，但裸引擎本身也叫 adapter 材料，边界混乱 |
| 新旧流程混杂 | legacy `process.rs` 276 行仅被 `lib.rs` 的 `start_task` 调用（前端已不调用，新入口 `start_multi_target_task` + `create_persistent_task`）；根 `ffmpeg.rs` 312 行仅被 `process.rs` 和 `e2e.rs` 使用 |
| 死代码 | `stt_whisper_candle.rs` 507 行（`lib.rs` 从未声明 `mod`，不编译）；`api_cosyvoice.rs` 14 行 stub（无调用）；`api_whisper_rs.rs` 169 行（有 `mod` 声明但无任何调用） |
| ZipVoice 半成品 | 前端配置字段已完成（`zipvoice_dir/prompt_wav/prompt_text/num_threads`），后端未接线；sherpa-onnx 已锁 1.13.5（含正式 ZipVoice Rust API，`OfflineTts` 声明了 unsafe Send/Sync）；官方 109MB int8 bundle 已下载；`register_tts` 选 zipvoice 报「不支持」 |

约束（不可违反）：

- Windows，页面文件/commit 紧张 → 所有重资源（STT/TTS/分离/视频合成）必须走 `scheduler` RAII 租约 + commit 预审，全局串行。
- 无 git。重构前已建 `.refactor-backup/` 全量源码备份；每个切片完成后必须测试全绿再进入下一片。
- 需求文档决策：`旧单目标流程暂保留作为回滚路径` —— **隔离不删**，删除属于未来的需求级决策，不在本轮。

---

## 2. 目标架构（分层与命名空间）

```text
src-tauri/src/
├── lib.rs                    # 仅 composition root：mod 声明、tauri builder、invoke_handler（目标 ≤ 150 行）
├── state.rs                  # RunningTask + TASKS 全局任务表（仅此一处）
├── types.rs                  # AppConfig / Segment / ProgressEvent（前后端对齐，serde default）
│
├── commands/                 # 全部 #[tauri::command]（唯一命令层）
│   ├── config.rs             # load_config / save_config / check_ffmpeg / pick_* / 文件读写 / open_path / get_log_dir / log_frontend
│   ├── api_test.rs           # test_api_endpoint / test_api_reachable
│   ├── task.rs               # start_multi_target_task / cancel_task / cancel_child_task
│   ├── tasks.rs              # 持久化任务 CRUD / 字幕读写 / 方言 / 模型下载
│   ├── media_tools.rs        # 媒体工具页命令
│   ├── runtime.rs            # get_runtime_info（模型状态）
│   ├── realtime_stt.rs       # 实时识别
│   └── diarization.rs        # 说话人分离
│
├── legacy/                   # 【冻结区】旧单目标流程，仅紧急回滚；禁止新代码依赖它
│   ├── command.rs            # start_task（IPC 名不变）
│   ├── process.rs            # 旧单目标 run()
│   └── ffmpeg.rs             # 旧 ffmpeg 工具（extract_audio / split_audio / mux_replaced_audio）
│
├── domain/                   # 纯类型：ParentTask/ChildTask/TargetVariant/Artifact/Manifest/Config
├── ports/                    # 纯 trait：SttEngine / Translator / TtsEngine / AudioSeparator / MediaTool
│
├── engines/                  # 【新增命名空间】裸引擎实现（非 port 实现，不含 stage 逻辑）
│   ├── stt/
│   │   ├── sensevoice.rs         # ← 原 stt_sensevoice.rs
│   │   ├── whisper_native.rs     # ← 原 stt_whisper_native.rs（candle）
│   │   ├── openai_api.rs         # ← 原 api_openai_stt.rs
│   │   └── whisper_cli.rs        # ← 原 api_whisper_local.rs
│   ├── tts/
│   │   └── supertonic/
│   │       ├── mod.rs            # 对外唯一入口（synthesize_segments / validate_language_assets 等，签名冻结）
│   │       ├── helper.rs         # ← 原 supertonic_helper.rs（ONNX/张量内部件，pub(in engines::tts::supertonic)）
│   │       └── assets.rs         # ← 原 tts_supertonic.rs 的资产校验部分（missing_*_files / official_available / zh_available）
│   └── translate/
│       └── deepseek.rs           # ← 原 api_deepseek.rs
│
├── adapters/                 # 仅 ports 的实现（裸引擎在此被包装成 port；不含裸引擎内部件）
│   ├── stt/{stage,legacy,sensevoice}.rs      # legacy.rs 改为引用 engines::stt::*
│   ├── tts/{stage,supertonic,cosyvoice3,zipvoice}.rs
│   ├── translate/{stage,openai_compatible}.rs
│   ├── separation/{stage,sherpa_uvr}.rs
│   └── media/{ffmpeg,stages,output_stages}.rs
│
├── application/              # pipeline_service / task_service / checkpoint / subtitle_edit / subtitle_import / dialects
├── infra/                    # 与 IO 绑定的实现
│   ├── task_store/{mod,store,index,atomic}.rs        # ← 原 task_store.rs 拆分
│   ├── artifact_store/{mod,store,paths,hash}.rs      # ← 原 artifact_store.rs 拆分
│   ├── api_client.rs / diskcheck.rs
│
├── pipeline/
│   ├── graph.rs / registry.rs / integration_tests.rs
│   └── runner/               # ← 原 runner.rs 拆分
│       ├── mod.rs            # re-export 门面（外部路径不变）
│       ├── types.rs          # RunScope / CancelToken / ArtifactInput / ExecutionContext / StageRequest / ArtifactOutput / ExecutionOutcome / ExecuteError / StageUpdate / StageRunResult / PipelineError
│       ├── records.rs        # stage_record / get_record(_mut) / insert_record / record_for_dependency / invalidate_record / validate_scope
│       ├── executor.rs       # PipelineRunner<E> 核心（字段顺序与 Drop 顺序冻结不动）
│       └── tests.rs          # 全部 #[cfg(test)] 测试（必须是 runner 的子模块，否则访问不到私有项）
│
├── scheduler.rs              # 资源调度（唯一知道资源的地方；不动）
├── memcheck.rs / logger.rs / segments.rs / subtitle*.rs / text_align.rs / audio_align.rs / audio_io.rs
├── ffmpeg.rs                 # 【删除】并入 legacy/ffmpeg.rs（e2e 改用 adapters::media::FfmpegTool）
└── e2e.rs                    # 真实模型 e2e（#[cfg(all(test, feature="inference"))]）
```

### 2.1 依赖方向（只允许向下）

```text
commands → application → pipeline → adapters → engines
   │           │                      │           │
   │           │                      └─ ports ───┘（双向解耦：adapters 实现 ports，engines 不依赖 ports）
   │           └─ infra（task_store/artifact_store/api_client）
   └─ legacy/ ── 只依赖 engines + infra + types；任何非 legacy 模块禁止依赖 legacy/
```

- `engines/` 不依赖 `ports/`：引擎是裸实现，port 语义只在 `adapters/` 注入。
- `adapters/` 依赖 `ports/` + `engines/`：把裸引擎包成 port 实现并声明资源成本。
- `legacy/` 可依赖 `engines/`、`infra/`、`types/`；**反向引用一律禁止**。

---

## 3. 关键设计决策（会诊 + 锦标赛结论）

1. **legacy 隔离不删**（C 方向，4 模型一致）：`process.rs + 根 ffmpeg.rs + start_task` 移入 `legacy/`，目录级标注「冻结，仅紧急回滚」。不加 feature flag —— 回滚路径必须持续编译才不 bit-rot。删除需满足全部移除标准（见 §6）。
2. **engines/ 而非 adapters/ 归位**（C1 > C2 的核心差异）：裸引擎进 `engines/`，`adapters/` 只剩 port 实现。避免 legacy → adapters 的依赖倒挂，且新增引擎的落点一目了然（engines 加实现、adapters 加包装、registry 加注册）。
3. **ZipVoice 独立切片、最后做**：它是功能变更不是结构重构，混入会污染「基线全绿」验证。接入方式：`adapters/tts/zipvoice.rs` 直接包装 sherpa-onnx `OfflineTts`（无需 engines/ 层），每个 TTS stage 在现有 RAII 租约后加载一次模型、同目标内复用；不做全局缓存（OfflineTts 非 Clone，全局缓存必然引入 unsafe 或专属 worker，超出本轮范围）。
4. **e2e 的 ffmpeg 依赖改走 adapters**：`e2e.rs` 仅用了根 `ffmpeg.rs::extract_audio`，改为 `adapters::media::ffmpeg::FfmpegTool::extract_audio`，使根 ffmpeg.rs 能完整进入 legacy/。
5. **测试拆分纪律**：先移 tests（保持 runner 子模块身份）→ types → records → executor core；可见性用 `pub(in crate::pipeline::runner)` 精确限定，**禁止**为拆文件把内部件升成 `pub(crate)`；`PipelineRunner` 字段顺序与 Drop 顺序一字不动。

### Rust 特有陷阱（评审共识，实施时逐项对照）

- **impl 块分裂后的可见性退化**：同文件私有互调失效 → 用 `pub(in crate::pipeline::runner)` 而非 `pub(crate)`。
- **跨方法 borrow 冲突（E0499）**：同一函数内 disjoint field borrow 跨方法后失效 → 拆分按字段所有权切，不按逻辑切；不把所有辅助都改成 `&mut self`。
- **RAII 租约与 Drop 时机**：租约移动到子方法会提前释放内存额度 → 租约变量保持在 executor 主方法作用域内。
- **Tauri `generate_handler!` 宏**：命令抽离后宏内路径必须同步改全；函数名（= IPC 名）一个字母不改；抽离后加一个命令清单核对（29 个）防止漏注册。
- **Tauri State 借用不跨 await**：commands 内先取数据/clone，再 `.await`。
- **Windows 路径**：命令参数用 `PathBuf` 不用 `String`。

---

## 4. 实施切片（每片独立可验证，顺序执行；✅ = 已完成）

| # | 切片 | 内容 | 验证门 |
|---|---|---|---|
| S0 | 备份与基线 | `.refactor-backup/` 全量源码备份；记录 182+27 基线 | ✅ 完成 |
| S1 | 死代码删除 | 删 `stt_whisper_candle.rs`（未编译）、`api_cosyvoice.rs`（无调用）、`api_whisper_rs.rs`（有 mod 无调用），同步删 lib.rs 两个 mod 声明 | cargo test 全绿 |
| S2 | lib.rs 抽离 | 16 个内联命令移入 `commands/{config,api_test,task}.rs`；`RunningTask`+`TASKS` 移到 `state.rs`；lib.rs ≤ 150 行；29 命令清单核对 | cargo test + npm test 全绿 |
| S3 | runner.rs 拆分 | `pipeline/runner/{mod,types,records,executor,tests}.rs`，mod.rs re-export 保外部路径 | cargo test 全绿 |
| S4 | 引擎归位 | 8 个裸引擎模块移入 `engines/{stt,tts,translate}/`；supertonic 三件套合并为 `engines/tts/supertonic/{mod,helper,assets}.rs`；adapters/legacy 仅改 import 路径，公开 API 签名冻结 | cargo test 全绿 |
| S5 | legacy 隔离 | `process.rs`+根`ffmpeg.rs`+`start_task` 移入 `legacy/`；`e2e.rs` 改用 `adapters::media::FfmpegTool`；需求文档标注冻结+移除标准 | cargo test 全绿 |
| S6 | ZipVoice 补全 | `adapters/tts/zipvoice.rs` + `register_tts` 分支 + runtime 模型状态 + preflight 校验 + 真实 109MB 模型生成验证 | cargo test 全绿 + 真实 TTS 出声 |
| S7 | infra 拆分 | `infra/task_store/{store,index,atomic}.rs`、`infra/artifact_store/{store,paths,hash}.rs` | cargo test 全绿 |

并行规则：S3、S7 可与 S2 并行（不同目录）；S4 必须在 S2 后（改 lib.rs mod 声明）；S5 必须在 S4 后（legacy 引用 engines 路径）；S6 必须在 S4 后（adapters/tts 结构稳定后接入）。

---

## 5. ZipVoice 切片（S6）设计要点

- 引擎：`sherpa_onnx::OfflineTts` + `OfflineTtsZipvoiceModelConfig`（tokens/encoder.int8.onnx/decoder.int8.onnx/vocoder(vocos_24khz)/espeak-ng-data/lexicon.txt），`num_steps=4`，`num_threads` 来自持久化配置（默认 2）。
- 模型目录：`AppConfig.zipvoice_dir`，preflight 校验必需文件，缺失在任务启动前报错（不走昂贵 STT）。
- 参考输入：`zipvoice_prompt_wav`（Wave::read）+ `zipvoice_prompt_text`，全局持久化；参考音频采样率按 `Wave::read` 实测值传给 `reference_sample_rate`。
- 资源：`resource_cost = scheduler::TTS`（重资源串行），模型在租约后加载一次、同目标全部字幕段复用；逐段生成后立即写盘（hound 流式），不驻留全长 buffer。
- 取消：段间检查 `CancelToken`；生成回调返回 false 请求中断；中断时丢弃当前段、保留已写段，正确 finalize WAV。
- 输出：与 supertonic/cosyvoice 一致的时间轴对齐逻辑（rubberband 0.85x~1.25x + 静音填补），产物 `targets/{variant_id}/dub.wav`。

---

## 6. legacy 移除标准（未来需求级决策，全部满足才可删）

- 连续 2 个发布版本无任何生产回滚使用；
- 新 pipeline 具备等价失败恢复能力；
- 新路径支持 legacy 全部引擎组合；
- 移除经过显式需求评审并更新 `FUNCTION_CHECKLIST.md`；
- 紧急 runbook 不再引用 `start_task`。

---

## 7. 重构后预期规模（估算）

| 指标 | 重构前 | 目标 |
|---|---|---|
| 最大文件行数 | runner.rs 1758 / lib.rs 915 | ≤ 800（executor.rs 核心，不含测试） |
| crate 根引擎文件 | 8 个 | 0 个 |
| 死代码 | 3 份 ~690 行 | 0 |
| lib.rs | 915 | ≤ 150 |
| Tauri 命令位置 | lib.rs 内联 16 个 | commands/ 唯一命令层 |
| 测试 | 182 + 27 全绿 | 同左 + ZipVoice 新增用例，不得减少语义覆盖 |

> 验证矩阵（每切片门禁 + 最终验收）：`cargo test`（默认 inference）、`cargo test --no-default-features`、`npm run test`、`npx vue-tsc --noEmit`、`npx vite build`。
