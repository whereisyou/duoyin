/** 应用配置（与 Rust 端 AppConfig 对齐） */
export interface AppConfig {
  // 语音识别
  stt_engine: string // sensevoice | whisper_native | whisper_local | openai_api
  sensevoice_dir: string
  whisper_model_dir: string
  whisper_cli_path: string
  whisper_model_path: string
  whisper_use_gpu: boolean
  openai_key: string
  // 翻译
  deepseek_key: string
  deepseek_model: string
  deepseek_api_url: string
  // 语音合成
  tts_engine: string // supertonic | cosyvoice3 | zipvoice
  supertonic_dir: string
  supertonic_voice: string
  cosyvoice_url: string
  cosyvoice_key: string
  cosyvoice_voice: string
  cosyvoice_prompt_wav: string
  cosyvoice_prompt_text: string
  cosyvoice_sample_rate: number
  // ZipVoice Distill INT8（本地零样本 TTS）
  zipvoice_dir: string
  zipvoice_prompt_wav: string
  zipvoice_prompt_text: string
  zipvoice_num_threads: number
  /** 配音克隆原视频音色：TTS 前自动从原声提取参考段（仅 ZipVoice 生效） */
  tts_use_video_prompt: boolean
  // 外部 API 调度
  http_proxy: string
  api_max_concurrent: number
  api_interval_ms: number
  // 主流程高级设置
  separation_enabled: boolean
  separator_model_path: string
  diarization_seg_model: string
  diarization_embedding_model: string
  diarization_num_speakers: number
  separation_denoise: boolean
  separation_normalize: boolean
  separation_fallback_no_bgm: boolean
  generate_final_videos: boolean
  output_naming: "source_variant" | "final"
  keep_original_audio_track: boolean
  min_speed_percent: number
  max_speed_percent: number
  subtitle_mode: "none" | "external_srt" | "hard_subtitle_planned"
  // 输出
  output_dir: string
}

export interface TargetVariant {
  id: string
  language: string
  dialect?: string
  display_name: string
  translate_style: string
  tts_accent: string
}

export interface DialectSpec {
  id: string
  label: string
  translate_style: string
  tts_accent: string
}

export interface LanguageDialectSpec {
  language: string
  dialects: DialectSpec[]
}

export interface PersistentChildSummary {
  variant_id: string
  status: string
  output_dir: string
  bytes: number
}

export interface PersistentTaskDetail {
  task_root: string
  task: {
    parent: { status: string }
    updated_at: number
  }
  manifest: {
    artifacts: Record<string, {
      kind: string
      status: string
      relative_path: string
      size: number
    }>
  }
  recovered_from_backup: boolean
}

export interface PersistentStageSummary {
  stage: string
  status: string
  error?: string
}

export interface PersistentTaskSummary {
  task_id: string
  status: string
  updated_at: number
  revision: number
  source_video: string
  source_language?: string
  targets: TargetVariant[]
  shared_stages: PersistentStageSummary[]
  shared_bytes: number
  task_root: string
  variant_bytes: Record<string, number>
  children: PersistentChildSummary[]
}

/** 任务配置（提交给后端） */
export interface TaskConfig {
  video: string
  source_lang: string
  target_lang: string
}

/** 字幕段 */
export type SubtitleEditMode = "source" | "target";

export interface SubtitleSegment {
  idx: number
  start: number
  end: number
  text: string
  translated?: string
}

/** 后端进度事件 */
export interface ProgressEvent {
  step: string
  progress: number
  status: string
  error?: string
  segments?: SubtitleSegment[]
  /** 完成时后端返回的实际输出目录 */
  output_dir?: string
  scope?: "parent" | "target"
  variant_id?: string
  parent_status?: string
}

export type TaskStatus = 'pending' | 'running' | 'done' | 'error' | 'canceled'

export interface ChildTaskItem {
  variant: TargetVariant
  status: TaskStatus
  step: string
  progress: number
  error?: string
  outputDir?: string
  bytes?: number
}

export interface SharedStageItem {
  stage: string
  status: TaskStatus
  error?: string
}

export interface ArtifactDetail {
  id: string
  relative_path: string
  size: number
  status: string
  kind: string
}

/** 前端任务项（队列中的一行） */
export interface TaskItem {
  /** 本地唯一 key（入队即生成） */
  key: string
  /** 后端任务 id（启动后才有） */
  backendId: string | null
  file: string
  fileName: string
  sourceLang: string
  targetLang: string
  targets: TargetVariant[]
  children: ChildTaskItem[]
  sharedStages: SharedStageItem[]
  status: TaskStatus
  /** 后端父任务聚合状态；用于区分全部失败与部分失败。 */
  parentStatus?: "completed" | "partially_failed" | "failed"
  /** 当前步骤标识，如 stt / translate */
  step: string
  progress: number
  error?: string
  segments?: SubtitleSegment[]
  createdAt: number
  startedAt?: number
  finishedAt?: number
  /** 完成后由后端返回的实际输出目录 */
  outputDir?: string
  sharedBytes?: number
  artifactDetails?: ArtifactDetail[]
  /** 运行日志（时间戳 + 文本） */
  logs: string[]
}
