import { reactive, computed } from "vue";
import {
  startMultiTargetTask,
  cancelTask,
  cancelChildTask,
  loadPersistentTask,
  deletePersistentTask,
  onTaskEvent,
} from "./api";
import type { RuntimeInfo } from "./api";
import type {
  AppConfig,
  TaskItem,
  SubtitleSegment,
  TargetVariant,
  LanguageDialectSpec,
  PersistentTaskSummary,
  SubtitleEditMode,
} from "./types";
import { languageVariant } from "./langs";

/** 步骤标识 → 中文名 */
export const STEP_LABELS: Record<string, string> = {
  media_probe: "媒体探测",
  extract_audio: "提取音频",
  separation: "背景音分离",
  stt: "语音识别",
  translate: "字幕翻译",
  tts: "合成配音",
  srt: "生成字幕",
  mix: "混合音频",
  final_video: "合成最终视频",
  done: "完成",
};

/** 真实 DAG 的展示顺序；背景分离与 STT 并行，目标阶段按各版本独立推进。 */
export const PIPELINE_STEPS = [
  "media_probe",
  "extract_audio",
  "separation",
  "stt",
  "translate",
  "tts",
  "mix",
  "srt",
  "final_video",
];

export type PageKey = "home" | "tasks" | "subtitle" | "tools" | "settings";

export const PAGE_LABELS: Record<PageKey, string> = {
  home: "工作台",
  tasks: "任务队列",
  subtitle: "字幕编辑",
  tools: "媒体工具",
  settings: "设置",
};

export const store = reactive({
  currentPage: "home" as PageKey,
  /** 来源页（供设置页等"返回"使用） */
  previousPage: null as PageKey | null,
  config: null as AppConfig | null,
  tasks: [] as TaskItem[],
  /** 最近一次完成任务的字幕（供字幕编辑页载入） */
  lastSegments: [] as SubtitleSegment[],
  dialectSpecs: [] as LanguageDialectSpec[],
  ffmpeg: { status: "checking" as "checking" | "ok" | "missing", version: "" },
  runtime: null as RuntimeInfo | null,
  /** 工作台会话状态：切页后保留文件列表与语言选择 */
  workbench: {
    stagedFiles: [] as string[],
    sourceLang: "auto",
    targetLangs: ["en"] as string[],
    dialectsByLanguage: {} as Record<string, string[]>,
    queueJustFinished: false,
  },
  /** 字幕编辑会话状态：切页后保留编辑内容 */
  subtitleEditor: {
    segments: [] as SubtitleSegment[],
    bilingual: true,
    taskId: null as string | null,
    variantId: null as string | null,
    mode: "target" as SubtitleEditMode,
    dirty: false,
  },
  /** 设置草稿：未保存的修改切页不丢失 */
  settings: {
    draft: null as AppConfig | null,
    dirty: false,
  },
});

/** 页面导航：记录来源页 */
export function restorePersistentTasks(summaries: PersistentTaskSummary[]): void {
  if (store.tasks.length > 0) return;
  store.tasks = summaries.map((summary) => ({
    key: `history_${summary.task_id}`,
    backendId: summary.task_id,
    file: summary.source_video,
    fileName: summary.source_video.split(/[\\/]/).pop() ?? summary.source_video,
    sourceLang: summary.source_language ?? "auto",
    targetLang: summary.targets.map((target) => target.id).join(","),
    targets: summary.targets,
    children: summary.targets.map((variant) => {
      const persisted = summary.children.find((child) => child.variant_id === variant.id);
      const childStatus = persisted?.status;
      const status: TaskItem["status"] =
        childStatus === "Completed"
          ? "done"
          : childStatus === "Failed"
            ? "error"
            : childStatus === "Canceled"
              ? "canceled"
              : "pending";
      return {
        variant,
        status,
        step: "",
        progress: status === "done" ? 100 : 0,
        outputDir: persisted?.output_dir,
        bytes: persisted?.bytes,
      };
    }),
    sharedStages: summary.shared_stages.map((stage) => ({
      stage: stage.stage,
      status:
        stage.status === "Done" || stage.status === "Skipped" || stage.status === "Degraded"
          ? "done"
          : stage.status === "Failed" || stage.status === "Interrupted"
            ? "error"
            : stage.status === "Running"
              ? "running"
              : "pending",
      error: stage.error,
    })), 
    parentStatus:
      summary.status === "PartiallyFailed"
        ? "partially_failed"
        : summary.status === "Completed"
          ? "completed"
          : summary.status === "Failed"
            ? "failed"
            : undefined,
    status:
      summary.status === "Completed"
        ? "done"
        : summary.status === "Failed" || summary.status === "PartiallyFailed"
          ? "error"
          : "pending",
    step: "",
    progress: summary.status === "Completed" ? 100 : 0,
    createdAt: summary.updated_at,
    finishedAt: summary.status === "Completed" ? summary.updated_at : undefined,
    outputDir: summary.task_root,
    sharedBytes: summary.shared_bytes,
    logs: [
      `[历史任务] 共享产物 ${formatBytes(summary.shared_bytes)} · 版本产物 ${formatBytes(
        Object.values(summary.variant_bytes).reduce((sum, value) => sum + value, 0),
      )}`,
    ],
  }));
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

export function editTaskSubtitles(
  task: TaskItem,
  mode: SubtitleEditMode,
  variantId?: string,
): void {
  if (!task.backendId) return;
  store.subtitleEditor.taskId = task.backendId;
  store.subtitleEditor.variantId = variantId ?? null;
  store.subtitleEditor.mode = mode;
  store.subtitleEditor.segments = [];
  store.subtitleEditor.dirty = false;
  navigateTo("subtitle");
}

export function navigateTo(page: PageKey): void {
  if (store.currentPage !== page) {
    store.previousPage = store.currentPage;
    store.currentPage = page;
  }
}

/** 返回来源页 */
export function goBack(): void {
  if (store.previousPage) {
    const target = store.previousPage;
    store.previousPage = null;
    store.currentPage = target;
  }
}

export const taskStats = computed(() => {
  const t = store.tasks;
  return {
    total: t.length,
    pending: t.filter((x) => x.status === "pending").length,
    running: t.filter((x) => x.status === "running").length,
    done: t.filter((x) => x.status === "done").length,
    error: t.filter((x) => x.status === "error").length,
  };
});

export const currentTask = computed(
  () => store.tasks.find((t) => t.status === "running") ?? null,
);

let queueRunning = false;
let seq = 0;

/** task.key → 完成信号：取消任务时主动唤醒 runOne，避免队列卡死 */
const doneResolvers = new Map<string, () => void>();

function now(): string {
  const d = new Date();
  const p = (n: number) => n.toString().padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

function log(task: TaskItem, text: string) {
  task.logs.push(`[${now()}] ${text}`);
  if (task.logs.length > 200) task.logs.splice(0, task.logs.length - 200);
}

const TARGET_STAGES = ["translate", "tts", "mix", "srt", "final_video"];

function refreshTaskProgress(task: TaskItem): void {
  const sharedDone = task.sharedStages.filter((stage) => stage.status === "done").length;
  const targetDone = task.children.reduce((sum, child) => {
    if (child.status === "done") return sum + TARGET_STAGES.length;
    const stageIndex = TARGET_STAGES.indexOf(child.step);
    return sum + Math.max(0, stageIndex) + (stageIndex >= 0 ? child.progress / 100 : 0);
  }, 0);
  const total = task.sharedStages.length + task.children.length * TARGET_STAGES.length;
  task.progress = total > 0 ? Math.min(100, Math.round(((sharedDone + targetDone) / total) * 100)) : 0;
}

/** 批量入队，返回入队数量 */
export function enqueueTasks(
  files: string[],
  sourceLang: string,
  targets: string | TargetVariant[],
): number {
  const targetVariants = typeof targets === "string" ? [languageVariant(targets)] : targets;
  if (targetVariants.length === 0) return 0;
  const exists = new Set(
    store.tasks
      .filter((t) => t.status === "pending" || t.status === "running")
      .map((t) => t.file),
  );
  let added = 0;
  for (const file of files) {
    if (exists.has(file)) continue;
    store.tasks.push({
      key: `t${Date.now()}_${seq++}`,
      backendId: null,
      file,
      fileName: file.split(/[\\/]/).pop() ?? file,
      sourceLang,
      targetLang: targetVariants.map((target) => target.id).join(","),
      targets: targetVariants.map((target) => ({ ...target })),
      children: targetVariants.map((variant) => ({
        variant: { ...variant },
        status: "pending" as const,
        step: "",
        progress: 0,
      })),
      sharedStages: ["media_probe", "extract_audio", "stt", "separation"].map((stage) => ({
        stage,
        status: "pending" as const,
      })),
      status: "pending",
      step: "",
      progress: 0,
      createdAt: Date.now(),
      logs: [],
    });
    added++;
  }
  return added;
}

/**
 * 启动队列：受限并发调度（最多 MAX_CONCURRENT 个任务同时在跑）。
 * 真正的资源互斥在后端 HEAVY 信号量（STT/TTS 全局唯一），
 * 所以这里并发开的任务：一个在做重资源阶段时，另一个正好可以等外部 API。
 */
const MAX_CONCURRENT = 2;
let activeTasks = 0;
let wakeScheduler: (() => void) | null = null;

export async function runQueue(): Promise<void> {
  if (queueRunning) return;
  queueRunning = true;
  try {
    for (;;) {
      const next = store.tasks.find((t) => t.status === "pending");
      if (!next) {
        if (activeTasks === 0) break;
      } else if (activeTasks < MAX_CONCURRENT) {
        activeTasks++;
        void runOne(next).finally(() => {
          activeTasks--;
          wakeScheduler?.();
        });
        continue;
      }
      // 队列满或没有待办：等任一任务结束再调度
      await new Promise<void>((r) => {
        wakeScheduler = r;
      });
      wakeScheduler = null;
    }
  } finally {
    queueRunning = false;
  }
}

async function runOne(task: TaskItem): Promise<void> {
  task.status = "running";
  task.startedAt = Date.now();
  task.step = "";
  task.progress = 0;
  task.error = undefined;
  for (const shared of task.sharedStages) {
    shared.status = "pending";
    shared.error = undefined;
  }
  for (const child of task.children) {
    child.status = "pending";
    child.step = "";
    child.progress = 0;
    child.error = undefined;
  }
  log(task, "任务启动");

  try {
    const id = await startMultiTargetTask(
      task.file,
      task.sourceLang,
      task.targets,
      task.backendId ?? undefined,
    );
    task.backendId = id;

    let settled = false;
    let resolveDone!: () => void;
    const done = new Promise<void>((r) => {
      resolveDone = () => {
        if (settled) return;
        settled = true;
        r();
      };
    });
    doneResolvers.set(task.key, resolveDone);

    const unlisten = await onTaskEvent(id, (evt) => {
      // 取消后迟到的事件直接忽略
      if (task.status === "canceled") return;
      if (evt.scope === "parent") {
        const stage = task.sharedStages.find((item) => item.stage === evt.step);
        if (stage) {
          stage.status = evt.status === "done" ? "done" : evt.status === "error" ? "error" : "running";
          stage.error = evt.error;
        }
      }
      if (evt.scope === "target" && evt.variant_id) {
        const child = task.children.find((item) => item.variant.id === evt.variant_id);
        if (child) {
          child.step = evt.step;
          child.progress = evt.progress;
          if (evt.status === "done" && evt.step === "done") child.status = "done";
          else if (evt.status === "error") {
            child.status = "error";
            child.error = evt.error;
          } else child.status = "running";
          if (evt.output_dir) child.outputDir = evt.output_dir;
          task.step = evt.step;
        }
      }
      const isParentTerminal =
        evt.scope === "parent" &&
        evt.step === "done" &&
        (evt.parent_status === "completed" || evt.parent_status === "partially_failed");
      refreshTaskProgress(task);
      if (evt.status === "done" && isParentTerminal) {
        task.parentStatus = evt.parent_status === "partially_failed" ? "partially_failed" : "completed";
        task.status = evt.parent_status === "partially_failed" ? "error" : "done";
        task.progress = 100;
        task.finishedAt = Date.now();
        if (evt.segments?.length) {
          task.segments = evt.segments;
          store.lastSegments = evt.segments;
        }
        task.outputDir = evt.output_dir;
        log(task, evt.parent_status === "partially_failed" ? "任务部分失败" : "任务完成");
        resolveDone();
        return;
      }
      if (evt.status === "error" && evt.scope === "parent") {
        task.parentStatus = "failed";
        task.status = "error";
        task.error = evt.error || "处理失败";
        task.finishedAt = Date.now();
        log(task, `失败：${task.error}`);
        resolveDone();
        return;
      }
      // 运行中进度
      if (evt.scope === "target") return;
      const isNewStep = evt.step !== task.step;
      task.step = evt.step;
      if (isNewStep) {
        log(task, `开始${STEP_LABELS[evt.step] ?? evt.step}`);
      }
    });

    // 后端可能在 start command 返回后、事件监听注册前已经完成。
    // 监听建立后再读取一次持久化终态，补上这个订阅竞态窗口。
    void loadPersistentTask(id)
      .then((detail) => {
        if (settled) return;
        const status = detail.task.parent.status;
        if (status === "Completed" || status === "PartiallyFailed") {
          task.parentStatus = status === "PartiallyFailed" ? "partially_failed" : "completed";
          task.status = status === "PartiallyFailed" ? "error" : "done";
          task.progress = 100;
          task.finishedAt = detail.task.updated_at;
          task.outputDir = detail.task_root;
          log(task, status === "PartiallyFailed" ? "任务部分失败" : "任务完成");
          resolveDone();
        } else if (status === "Failed") {
          task.parentStatus = "failed";
          task.status = "error";
          task.error = "处理失败";
          task.finishedAt = detail.task.updated_at;
          resolveDone();
        }
      })
      .catch(() => {
        // 实时事件仍是主通道；兜底查询失败不应中断正在运行的任务。
      });

    await done;
    unlisten();
  } catch (e) {
    task.status = "error";
    task.error = String(e);
    task.finishedAt = Date.now();
    log(task, `失败：${task.error}`);
  } finally {
    doneResolvers.delete(task.key);
  }
}

/** 取消：排队中直接标记，运行中调用后端取消 */
export async function cancelTaskItem(task: TaskItem): Promise<void> {
  if (task.status === "pending") {
    task.status = "canceled";
    task.finishedAt = Date.now();
    log(task, "已取消");
    return;
  }
  if (task.status === "running" && task.backendId) {
    try {
      await cancelTask(task.backendId);
    } catch {
      /* 后端句柄可能已释放 */
    }
    task.status = "canceled";
    task.finishedAt = Date.now();
    log(task, "已取消");
    // 后端被 abort 后不会再发事件，主动唤醒 runOne 让队列继续
    doneResolvers.get(task.key)?.();
  }
}

export async function cancelChildTaskItem(task: TaskItem, variantId: string): Promise<void> {
  if (!task.backendId) return;
  const child = task.children.find((item) => item.variant.id === variantId);
  if (!child || child.status === "done" || child.status === "error") return;
  await cancelChildTask(task.backendId, variantId);
  child.status = "canceled";
  child.error = undefined;
  log(task, `已取消 ${child.variant.display_name}`);
}

/** 重试：重置为排队状态并启动队列 */
export function retryTaskItem(task: TaskItem): void {
  task.status = "pending";
  // 保留 backendId，后端才能在原任务目录 reconcile 并复用用户编辑/已完成产物。
  // 只有从未启动过的前端排队任务才会自然保持 null 并创建新任务。
  task.step = "";
  task.progress = 0;
  task.error = undefined;
  task.parentStatus = undefined;
  task.logs = [];
  void runQueue();
}

export async function removeTaskItem(task: TaskItem): Promise<void> {
  // 先落盘删除：磁盘目录 + 索引条目一并移除，重启不再复活；失败由调用方提示，本地不删
  if (task.backendId) {
    await deletePersistentTask(task.backendId);
  }
  const i = store.tasks.indexOf(task);
  if (i >= 0) store.tasks.splice(i, 1);
}

export async function clearFinishedTasks(): Promise<void> {
  const finished = store.tasks.filter(
    (t) => t.status !== "pending" && t.status !== "running",
  );
  await Promise.all(
    finished
      .filter((t) => t.backendId)
      .map((t) => deletePersistentTask(t.backendId as string)),
  );
  store.tasks = store.tasks.filter(
    (t) => t.status === "pending" || t.status === "running",
  );
}
