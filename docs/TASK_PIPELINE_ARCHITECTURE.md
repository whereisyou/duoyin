# 可恢复多版本任务流水线架构设计

> 依据：`docs/FUNCTION_CHECKLIST.md`。  
> 目标：在不急于写真实功能的前提下，先定义稳定的数据模型、产物复用规则、父子任务关系和 fake pipeline 测试边界。  
> 范围：视频翻译合成新视频主流程；支持背景音分离、多语言/多方言、多版本产物、历史恢复。

---

## 1. 设计目标

### 1.1 必须满足

1. 一个源视频只做一次父级共享工作：媒体探测、音频提取、可选背景音分离、STT。
2. 多语言/多方言拆成多个子任务：每个目标版本单独翻译、TTS、混音、可选视频。
3. 中间产物持久化，能最大限度复用，避免重复消耗时间/算力/API。
4. 应用重启后能恢复任务历史、继续未完成任务。
5. 用户编辑/替换产物后，只 invalidates 受影响的下游阶段。
6. 背景音分离与 STT 可并行，受调度器控制资源。
7. 架构允许后续引擎像积木一样增加，不改流水线核心。

### 1.2 当前不做

- 每目标版本独立翻译模型/API。
- 每目标版本独立 TTS 引擎/音色。
- 每语言单独勾选是否输出最终视频。
- 手动“重新校验产物”按钮。
- 硬字幕样式编辑。
- 多角色配音角色→音色映射。

---

## 2. 目录结构

任务根目录固定在应用数据目录：

```text
%LOCALAPPDATA%/videotrans/tasks/{parent_task_id}/
  task.json
  manifest.json
  source.json
  media.json
  audio.wav
  vocals.raw.wav
  vocals.normalized.wav        # 可选
  bgm.raw.wav
  bgm.normalized.wav           # 可选
  stt/
    segments.json
  targets/
    zh-CN/
      variant.json
      translated.json
      translated.srt
      dub.wav
      mixed.wav
      final.mp4                # 可选
    zh-yue/
      variant.json
      translated.json
      translated.srt
      dub.wav
      mixed.wav
      final.mp4                # 可选
```

### 2.1 命名原则

- 父级共享产物放父任务根目录或 `stt/`。
- 目标版本产物全部放 `targets/{variant_id}/`。
- 所有可复用文件都必须在 manifest 中登记。
- 所有写入采用 `.tmp` → flush/sync → atomic rename。

---

## 3. 核心数据模型

### 3.1 ParentTask

```rust
struct ParentTask {
    id: TaskId,
    source: SourceVideo,
    status: ParentStatus,
    created_at: DateTime,
    updated_at: DateTime,
    shared_stage: SharedStageState,
    targets: Vec<ChildTask>,
    settings_snapshot: ParentSettings,
}
```

状态：

```rust
enum ParentStatus {
    Pending,
    Running,
    Interrupted,
    PartialFailed,
    Failed,
    Done,
    Canceled,
}
```

规则：

- STT 失败 → 父任务 Failed，全部子任务阻塞。
- 任一子任务失败但其他仍可完成 → ParentStatus::PartialFailed。
- 用户重启应用时，Running → Interrupted。

### 3.2 ChildTask

```rust
struct ChildTask {
    id: ChildTaskId,
    parent_id: TaskId,
    variant: TargetVariant,
    status: ChildStatus,
    stage_state: TargetStageState,
}
```

状态：

```rust
enum ChildStatus {
    Pending,
    Running,
    Failed,
    Done,
    Canceled,
    Invalidated,
}
```

规则：

- 子任务允许单独取消，不影响其他子任务。
- 某目标版本翻译失败，其他目标版本继续。
- 子任务第一版只保证取消、失败展示、打开产物目录；重新 TTS/导入字幕等更细操作按文档归类。

### 3.3 TargetVariant

```rust
struct TargetVariant {
    id: String,              // zh-CN / zh-yue / en
    language: String,        // zh / en / ...
    dialect: Option<String>, // mandarin / yue / ...
    display_name: String,    // 中文（粤语）
    translate_style: String,
    tts_accent: String,
}
```

初始中文方言：

```text
普通话、广东话/粤语、东北话、甘肃话、贵州话、河南话、湖北话、湖南话、江西话、闽南话、宁夏话、山西话、陕西话、山东话、上海话、四川话、天津话、云南话
```

方言配置：

```text
内置默认 JSON + 应用同级 config/dialects.json 扩展
```

---

## 4. Manifest 模型

### 4.1 Manifest

```rust
struct TaskManifest {
    schema_version: u32,
    app_version: String,
    parent_task_id: TaskId,
    source_fingerprint: SourceFingerprint,
    stages: BTreeMap<StageId, StageRecord>,
    target_stages: BTreeMap<TargetVariantId, BTreeMap<StageId, StageRecord>>,
}
```

### 4.2 StageRecord

```rust
struct StageRecord {
    status: StageStatus,
    dependency_hash: String,
    artifacts: Vec<ArtifactRecord>,
    started_at: Option<DateTime>,
    completed_at: Option<DateTime>,
    error: Option<String>,
}
```

### 4.3 ArtifactRecord

```rust
struct ArtifactRecord {
    name: String,
    relative_path: PathBuf,
    kind: ArtifactKind,
    size: u64,
    modified: i64,
    hash: Option<String>,
}
```

`hash` 可按阶段策略决定：

- 大文件优先 size+mtime 快校验。
- 关键 JSON/SRT 建议 hash。
- 用户手动修改后 size/mtime 变化即可触发重新判断。

### 4.4 StageStatus

```rust
enum StageStatus {
    Pending,
    Running,
    Done,
    Failed,
    Skipped,
    Invalidated,
}
```

---

## 5. 阶段图

### 5.1 父级共享阶段

```text
media_probe
  → extract_audio
  ├─ stt
  └─ separation   # 用户开启时，与 stt 并行
```

父级产物：

| 阶段 | 产物 |
|---|---|
| media_probe | `media.json` |
| extract_audio | `audio.wav` |
| separation | `vocals.raw.wav`, `bgm.raw.wav`, optional normalized files |
| stt | `stt/segments.json` |

### 5.2 子任务阶段

```text
translate
  → tts
  → mix_audio
  → srt
  → final_video?   # 全局开关开启时
```

子任务产物：

| 阶段 | 产物 |
|---|---|
| translate | `translated.json`, `translated.srt` |
| tts | `dub.wav` |
| mix_audio | `mixed.wav` |
| srt | `translated.srt` |
| final_video | `final.mp4` |

---

## 6. DependencyHash 规则

### 6.1 父级阶段

| 阶段 | dependency_hash 输入 |
|---|---|
| media_probe | source path + size + mtime + probe_schema_version |
| extract_audio | source_fingerprint + audio_extract_settings |
| separation | audio_artifact + separation_model_id + separation_settings |
| stt | audio_artifact + stt_engine_id + stt_model_id + source_language |

> 注意：背景音分离不作为 STT 前置依赖。STT 默认使用 `audio.wav`，不是 `vocals.wav`。

### 6.2 子任务阶段

| 阶段 | dependency_hash 输入 |
|---|---|
| translate | `segments.json` + variant.language + variant.dialect + translate_prompt_version + translate_engine_id |
| tts | `translated.json` + variant.tts_accent + tts_engine_id + tts_settings |
| mix_audio | `dub.wav` + `bgm.wav` if exists + mix_settings |
| srt | `translated.json` + srt_schema_version |
| final_video | source_video + `mixed.wav/dub.wav` + subtitle_mode + video_mux_settings |

### 6.3 失效规则

- 编辑父级 STT 原文：全部 target 的 translate/tts/mix/final 失效。
- 编辑某语言译文：仅该 variant 的 tts/mix/final 失效。
- 删除必要文件：对应阶段和下游阶段待重跑。
- 替换外部 SRT：该 variant 的 translate 视为 Done，但 tts/mix/final 失效。

---

## 7. 产物复用算法

```rust
fn can_reuse(stage: StageRecord, current_dependency_hash: &str) -> bool {
    stage.status == Done
        && stage.dependency_hash == current_dependency_hash
        && all_artifacts_exist_and_match(stage.artifacts)
}
```

执行流程：

```text
for stage in graph_order:
  if can_reuse(stage):
    emit skipped
    load artifacts
  else:
    mark stage and downstream invalidated
    run stage
    write artifacts atomically
    update manifest
```

### 7.1 原子写入

```text
file.tmp → flush/sync → rename(file)
manifest.tmp → flush/sync → rename(manifest.json)
```

Crash 后：

- `.tmp` 文件忽略或清理。
- `Running` 阶段改为 `Pending/Interrupted`。
- 不允许把未登记为 Done 的文件当有效产物。

---

## 8. 资源调度设计

### 8.1 资源成本

```rust
struct Cost {
    cpu: u32,
    ram_bytes: u64,
    api_group: Option<String>,
}
```

本地重资源阶段：

- STT
- separation
- TTS
- final_video（ffmpeg/mux 可按较轻资源处理）

外部 API 阶段：

- translate
- external STT
- external TTS

### 8.2 调度原则

- API 阶段不占本地重资源许可。
- 本地重资源阶段由 scheduler 准入。
- 分离与 STT 可并行的前提：调度器判断资源足够；资源不足则排队。
- 多目标子任务由调度器自动入队，不要求用户手工决定顺序。

---

## 9. UI 架构

### 9.1 主界面保持简单

主界面只保留：

- 原语言（可未知/自动）
- 翻译语言多选
- 开始任务
- 底部任务摘要

高级能力入口：

- 目标语言右侧齿轮 → 方言/高级语言选项
- 背景音分离二级菜单
- 视频合成高级设置二级菜单

### 9.2 父任务详情

父任务详情展示：

```text
共享产物：
  media_probe / audio.wav / bgm.wav / vocals.wav / segments.json

目标版本：
  中文（普通话） 总大小 120MB [展开]
    translated.json
    translated.srt
    dub.wav
    mixed.wav
    final.mp4
  中文（粤语） 失败：翻译 API 限流
```

父任务状态：

- Running
- PartialFailed
- Done
- Failed
- Interrupted

### 9.3 硬字幕占位

视频合成高级设置中显示：

```text
硬字幕烧录：后续支持
```

不可作为主界面选项。

---

## 10. Fake Pipeline 测试计划

先写 fake pipeline，不碰真实模型/API。

### 10.1 必测路径

1. 首次完整运行：父任务 + 2 个目标版本。
2. 重启恢复：`audio.wav`、`segments.json` 已存在，跳过父级阶段。
3. 背景音分离与 STT 并行：资源允许时并行，资源不足时排队。
4. 某个子任务翻译失败：其他子任务继续，父任务 PartialFailed。
5. 编辑父级 STT 原文：所有目标版本 translate 之后失效。
6. 编辑单语言译文：仅该语言 tts/mix/final 失效。
7. 删除 `dub.wav`：仅该语言 TTS 之后待重跑。
8. 替换某语言外部 SRT：跳过 translate，重跑 TTS。
9. Crash 中断：Running → Interrupted，`.tmp` 文件不复用。
10. 全局关闭输出最终视频：只生成并保留音频/字幕。

### 10.2 测试优先级

P0：manifest 校验、恢复、失效传播。  
P1：调度并行/排队。  
P2：UI 状态映射。

---

## 11. 分阶段落地

### Phase 1：数据模型与 fake pipeline

- `task_manifest.rs`
- `artifact.rs`
- `target_variant.rs`
- `task_store.rs`
- fake pipeline 测试

### Phase 2：真实父级共享阶段

- 稳定任务目录
- 媒体探测
- 音频提取
- STT 复用
- 背景音分离占位/模型接入

### Phase 3：真实子任务阶段

- 多目标翻译
- 多目标 TTS
- 多目标字幕/音频保存
- 父子任务 UI

### Phase 4：视频合成

- 混音
- 默认替换音轨合成
- 外挂 SRT 文件输出
- final.mp4 输出

### Phase 5：高级/后续

- 重跑所有目标语言
- 手动重新校验产物
- 更多分离模型
- 硬字幕/ASS 样式
- 多角色配音
