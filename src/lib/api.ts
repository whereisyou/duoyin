import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AppConfig,
  TaskConfig,
  ProgressEvent,
  TargetVariant,
  LanguageDialectSpec,
  PersistentTaskSummary,
  PersistentTaskDetail,
} from "./types";

export async function startTask(config: TaskConfig): Promise<string> {
  return invoke("start_task", { config });
}

export async function startMultiTargetTask(
  video: string,
  sourceLanguage: string,
  targets: TargetVariant[],
  existingTaskId?: string,
): Promise<string> {
  return invoke("start_multi_target_task", {
    video,
    sourceLanguage,
    targets,
    existingTaskId: existingTaskId ?? null,
  });
}

export async function loadDialectSpecs(): Promise<LanguageDialectSpec[]> {
  return invoke("load_dialect_specs");
}

export async function listPersistentTasks(): Promise<PersistentTaskSummary[]> {
  return invoke("list_persistent_tasks");
}

export async function loadPersistentTask(taskId: string): Promise<PersistentTaskDetail> {
  return invoke("load_persistent_task", { taskId });
}

export async function deletePersistentTask(taskId: string): Promise<void> {
  return invoke("delete_persistent_task", { taskId });
}

export async function loadTaskSegments(
  taskId: string,
  variantId?: string,
): Promise<import("./types").SubtitleSegment[]> {
  return invoke("load_task_segments", { taskId, variantId: variantId ?? null });
}

export async function saveTaskSegments(
  taskId: string,
  variantId: string | undefined,
  segments: import("./types").SubtitleSegment[],
): Promise<void> {
  return invoke("save_task_segments", { taskId, variantId: variantId ?? null, segments });
}

export async function importTargetSrt(taskId: string, variantId: string): Promise<void> {
  return invoke("import_target_srt", { taskId, variantId });
}

export async function cancelTask(id: string): Promise<void> {
  return invoke("cancel_task", { id });
}

export async function cancelChildTask(id: string, variantId: string): Promise<void> {
  return invoke("cancel_child_task", { id, variantId });
}

export async function loadConfig(): Promise<AppConfig> {
  return invoke("load_config");
}

export async function saveConfig(config: AppConfig): Promise<void> {
  return invoke("save_config", { config });
}

export interface RuntimeInfo {
  gpu?: string
  models: { id: string; ready: boolean; path: string; bytes: number }[]
}

export async function transcribeAudioChunk(
  bytes: number[],
  extension: string,
  language: string,
): Promise<import("./types").SubtitleSegment[]> {
  return invoke("transcribe_audio_chunk", { bytes, extension, language });
}

export async function runSpeakerDiarization(
  taskId: string,
): Promise<{ segment_idx: number; speaker: string }[]> {
  return invoke("run_speaker_diarization", { taskId });
}

export async function getRuntimeInfo(): Promise<RuntimeInfo> {
  return invoke("get_runtime_info");
}

export async function matchTextToSrt(
  srtPath: string,
  textPath: string,
  output: string,
): Promise<void> {
  return invoke("match_text_to_srt", { srtPath, textPath, output });
}

export async function clipVideo(
  input: string,
  output: string,
  startSeconds: number,
  endSeconds: number,
): Promise<void> {
  return invoke("clip_video", { input, output, startSeconds, endSeconds });
}

export async function separateMedia(
  input: string,
  outputDir: string,
): Promise<{ video: string; audio: string }> {
  return invoke("separate_media", { input, outputDir });
}

export async function mergeVideoAudio(video: string, audio: string, output: string): Promise<void> {
  return invoke("merge_video_audio", { video, audio, output });
}

export async function checkFfmpeg(): Promise<string> {
  return invoke("check_ffmpeg");
}

export async function ensureUvrModel(): Promise<string> {
  return invoke("ensure_uvr_model");
}

export async function pickOnnxModel(): Promise<string | null> {
  return invoke("pick_onnx_model");
}

/** 选择多个视频文件 */
export async function pickVideoFiles(): Promise<string[]> {
  return invoke("pick_video_files");
}

/** 写文本文件（用于导出 SRT） */
export async function writeTextFile(path: string, content: string): Promise<void> {
  return invoke("write_text_file", { path, content });
}

/** 读文本文件（用于导入 SRT） */
export async function readTextFile(path: string): Promise<string> {
  return invoke("read_text_file", { path });
}

/** 测试 API 端点连通性（chat/completions，验证鉴权+模型）；成功返回耗时字符串，失败抛错 */
export async function testApiEndpoint(
  url: string,
  apiKey: string,
  model: string,
): Promise<string> {
  return invoke("test_api_endpoint", { url, apiKey, model });
}

/** 通用可达性测试（非 chat 接口：Whisper/CosyVoice 等）；任意 HTTP 响应即算通路 */
export async function testApiReachable(url: string, apiKey: string): Promise<string> {
  return invoke("test_api_reachable", { url, apiKey });
}

/** 前端异常转发到后端日志（fire-and-forget，失败静默防循环） */
export function logFrontend(level: string, message: string): void {
  invoke("log_frontend", { level, message }).catch(() => {});
}

/** 获取后端日志目录 */
export async function getLogDir(): Promise<string> {
  return invoke("get_log_dir");
}

/** 在系统文件管理器中打开路径 */
export async function openPath(path: string): Promise<void> {
  return invoke("open_path", { path });
}

export function onTaskEvent(
  taskId: string,
  callback: (event: ProgressEvent) => void,
): Promise<() => void> {
  return listen<ProgressEvent>(`task:${taskId}`, (event) => callback(event.payload));
}
