/**
 * 队列调度行为测试：这是「莫名报错/队列卡死」最容易出问题的地方，
 * 类型检查完全覆盖不到——必须用行为测试锁住并发上限、取消唤醒、错误续跑。
 */
import { describe, it, expect, vi, beforeEach } from "vitest";

// mock 必须在 import store 之前生效：vi.hoisted 保证工厂里引用的 mock 已就绪
const mocks = vi.hoisted(() => ({
  startTask: vi.fn(),
  startMultiTargetTask: vi.fn(),
  cancelTask: vi.fn(),
  cancelChildTask: vi.fn(),
  loadPersistentTask: vi.fn(),
  deletePersistentTask: vi.fn(),
  onTaskEvent: vi.fn(),
}));
vi.mock("../api", () => ({
  startTask: mocks.startTask,
  startMultiTargetTask: mocks.startMultiTargetTask,
  cancelTask: mocks.cancelTask,
  cancelChildTask: mocks.cancelChildTask,
  loadPersistentTask: mocks.loadPersistentTask,
  deletePersistentTask: mocks.deletePersistentTask,
  onTaskEvent: mocks.onTaskEvent,
}));

import {
  store,
  enqueueTasks,
  runQueue,
  cancelTaskItem,
  cancelChildTaskItem,
  retryTaskItem,
  clearFinishedTasks,
  taskStats,
  restorePersistentTasks,
  STEP_LABELS,
  PIPELINE_STEPS,
} from "../store";

/** 让挂起的微任务/定时器跑完 */
const flush = () => new Promise((r) => setTimeout(r, 0));

/** 每个后端 id 的事件回调 */
let cbs: Map<string, (e: any) => void>;
let nextId: number;

beforeEach(() => {
  store.tasks = [];
  store.lastSegments = [];
  cbs = new Map();
  nextId = 0;
  mocks.startTask.mockReset();
  mocks.startMultiTargetTask.mockReset();
  mocks.cancelTask.mockReset();
  mocks.cancelChildTask.mockReset();
  mocks.loadPersistentTask.mockReset();
  mocks.deletePersistentTask.mockReset();
  mocks.onTaskEvent.mockReset();
  mocks.startTask.mockImplementation(() => Promise.resolve(`id${++nextId}`));
  mocks.startMultiTargetTask.mockImplementation((_video, _source, _targets, existingId) =>
    Promise.resolve(existingId ?? `id${++nextId}`),
  );
  mocks.loadPersistentTask.mockImplementation(() => new Promise(() => {}));
  mocks.deletePersistentTask.mockImplementation(() => Promise.resolve());
  mocks.onTaskEvent.mockImplementation((id: string, cb: (e: any) => void) => {
    cbs.set(id, cb);
    return Promise.resolve(() => {}); // unlisten
  });
});

async function fireDone(id: string) {
  cbs.get(id)!({
    step: "done",
    scope: "parent",
    parent_status: "completed",
    status: "done",
    progress: 100,
    segments: [],
    output_dir: "",
  });
  await flush();
}

describe("enqueueTasks", () => {
  it("入队生成任务项并按文件名派生 fileName", () => {
    const n = enqueueTasks(["C:/a/x.mp4", "D:/b/y.mkv"], "en", "zh");
    expect(n).toBe(2);
    expect(store.tasks).toHaveLength(2);
    expect(store.tasks[0].fileName).toBe("x.mp4");
    expect(store.tasks[0].status).toBe("pending");
  });

  it("多目标入队保留一个父任务和多个子版本", () => {
    const n = enqueueTasks(
      ["a.mp4"],
      "auto",
      [
        { id: "en", language: "en", display_name: "英语", translate_style: "", tts_accent: "" },
        { id: "ja", language: "ja", display_name: "日语", translate_style: "", tts_accent: "" },
      ],
    );
    expect(n).toBe(1);
    expect(store.tasks).toHaveLength(1);
    expect(store.tasks[0].children).toHaveLength(2);
  });

  it("对 pending/running 中的相同文件去重", () => {
    enqueueTasks(["a.mp4"], "en", "zh");
    const added = enqueueTasks(["a.mp4", "b.mp4"], "en", "zh");
    expect(added).toBe(1);
    expect(store.tasks).toHaveLength(2);
  });
});

describe("runQueue 受限并发调度", () => {
  it("最多同时启动 MAX_CONCURRENT(2) 个任务，完成一个才补一个", async () => {
    enqueueTasks(["a.mp4", "b.mp4", "c.mp4", "d.mp4"], "en", "zh");
    const done = runQueue();
    await flush();

    // 只应先启动 2 个
    expect(mocks.startMultiTargetTask).toHaveBeenCalledTimes(2);

    await fireDone("id1"); // 完成第 1 个 → 补第 3 个
    expect(mocks.startMultiTargetTask).toHaveBeenCalledTimes(3);

    await fireDone("id2");
    expect(mocks.startMultiTargetTask).toHaveBeenCalledTimes(4);

    await fireDone("id3");
    await fireDone("id4");
    await done; // 队列必须能正常结束（卡死则超时失败）

    expect(store.tasks.every((t) => t.status === "done")).toBe(true);
    expect(taskStats.value.done).toBe(4);
  });

  it("子版本中间阶段完成不会被误判为该版本全部完成", async () => {
    enqueueTasks(["multi.mp4"], "auto", [
      { id: "en", language: "en", display_name: "英语", translate_style: "", tts_accent: "" },
    ]);
    const done = runQueue();
    await flush();

    cbs.get("id1")!({
      scope: "target",
      variant_id: "en",
      step: "translate",
      status: "done",
      progress: 100,
    });
    expect(store.tasks[0].children[0].status).toBe("running");
    expect(store.tasks[0].children[0].step).toBe("translate");

    await fireDone("id1");
    await done;
  });

  it("父级中间阶段完成不会被误判为整个任务完成", async () => {
    enqueueTasks(["a.mp4", "b.mp4", "c.mp4"], "en", "zh");
    const done = runQueue();
    await flush();
    expect(mocks.startMultiTargetTask).toHaveBeenCalledTimes(2);

    cbs.get("id1")!({
      step: "media_probe",
      scope: "parent",
      parent_status: "running",
      status: "done",
      progress: 100,
    });
    await flush();
    expect(store.tasks[0].status).toBe("running");
    expect(mocks.startMultiTargetTask).toHaveBeenCalledTimes(2);

    await fireDone("id1");
    expect(mocks.startMultiTargetTask).toHaveBeenCalledTimes(3);
    await fireDone("id2");
    await fireDone("id3");
    await done;
  });

  it("多目标只启动一个后端父任务，不重复执行 STT", async () => {
    enqueueTasks(
      ["multi.mp4"],
      "auto",
      [
        { id: "en", language: "en", display_name: "英语", translate_style: "", tts_accent: "" },
        { id: "ja", language: "ja", display_name: "日语", translate_style: "", tts_accent: "" },
      ],
    );
    const done = runQueue();
    await flush();
    expect(mocks.startMultiTargetTask).toHaveBeenCalledTimes(1);
    expect(mocks.startTask).not.toHaveBeenCalled();
    cbs.get("id1")!({ scope: "target", variant_id: "en", status: "done", step: "done", progress: 100 });
    cbs.get("id1")!({ scope: "target", variant_id: "ja", status: "error", step: "error", progress: 0, error: "tts failed" });
    cbs.get("id1")!({ scope: "parent", parent_status: "partially_failed", status: "done", step: "done", progress: 100, error: "1 个目标版本失败" });
    await done;
    expect(store.tasks[0].children[0].status).toBe("done");
    expect(store.tasks[0].children[1].status).toBe("error");
    expect(store.tasks[0].status).toBe("error");
  });

  it("单个任务出错不阻塞队列，其余继续", async () => {
    enqueueTasks(["a.mp4", "b.mp4"], "en", "zh");
    const done = runQueue();
    await flush();
    cbs.get("id1")!({ scope: "parent", step: "pipeline", status: "error", progress: 0, error: "boom" });
    await flush();
    await fireDone("id2");
    await done;
    expect(store.tasks[0].status).toBe("error");
    expect(store.tasks[0].error).toBe("boom");
    expect(store.tasks[1].status).toBe("done");
  });

  it("重复调用 runQueue 不会二次启动同一任务", async () => {
    enqueueTasks(["a.mp4"], "en", "zh");
    const p1 = runQueue();
    void runQueue(); // 并发再调一次
    await flush();
    expect(mocks.startMultiTargetTask).toHaveBeenCalledTimes(1);
    await fireDone("id1");
    await p1;
  });
});

describe("取消与重试", () => {
  it("取消 pending 任务：不调后端，直接标记 canceled", async () => {
    enqueueTasks(["a.mp4"], "en", "zh");
    await cancelTaskItem(store.tasks[0]);
    expect(store.tasks[0].status).toBe("canceled");
    expect(mocks.cancelTask).not.toHaveBeenCalled();
  });

  it("取消 running 任务：调后端取消并唤醒队列继续", async () => {
    enqueueTasks(["a.mp4", "b.mp4"], "en", "zh");
    const done = runQueue();
    await flush();
    // 第 1 个已在运行（backendId=id1）
    await cancelTaskItem(store.tasks[0]);
    await flush();
    expect(mocks.cancelTask).toHaveBeenCalledWith("id1");
    expect(store.tasks[0].status).toBe("canceled");
    await fireDone("id2");
    await done;
    expect(store.tasks[1].status).toBe("done");
  });

  it("取消单个子版本不取消父任务和其他版本", async () => {
    enqueueTasks(
      ["multi.mp4"],
      "auto",
      [
        { id: "en", language: "en", display_name: "英语", translate_style: "", tts_accent: "" },
        { id: "ja", language: "ja", display_name: "日语", translate_style: "", tts_accent: "" },
      ],
    );
    const task = store.tasks[0];
    task.status = "running";
    task.backendId = "parent1";
    task.children[0].status = "running";
    task.children[1].status = "pending";

    await cancelChildTaskItem(task, "en");

    expect(mocks.cancelChildTask).toHaveBeenCalledWith("parent1", "en");
    expect(mocks.cancelTask).not.toHaveBeenCalled();
    expect(task.status).toBe("running");
    expect(task.children[0].status).toBe("canceled");
    expect(task.children[1].status).toBe("pending");
  });

  it("重试沿用原任务 ID，复用已完成和用户编辑产物", async () => {
    enqueueTasks(["a.mp4"], "en", "zh");
    const task = store.tasks[0];
    task.backendId = "persisted-task";
    task.status = "error";

    retryTaskItem(task);
    await flush();

    expect(mocks.startMultiTargetTask).toHaveBeenCalledWith(
      "a.mp4",
      "en",
      task.targets,
      "persisted-task",
    );
    await fireDone("persisted-task");
    await flush();
    expect(task.status).toBe("done");
  });

  it("重试：错误任务重置为 pending 并重新跑完", async () => {
    enqueueTasks(["a.mp4"], "en", "zh");
    const done = runQueue();
    await flush();
    cbs.get("id1")!({ scope: "parent", step: "pipeline", status: "error", progress: 0, error: "x" });
    await flush();
    await done;
    expect(store.tasks[0].status).toBe("error");

    retryTaskItem(store.tasks[0]);
    await flush();
    expect(store.tasks[0].status).toBe("running");
    expect(mocks.startMultiTargetTask).toHaveBeenLastCalledWith(
      "a.mp4",
      "en",
      store.tasks[0].targets,
      "id1",
    );
    await fireDone("id1");
    // retry 内部 runQueue 完成后任务应为 done
    await flush();
    expect(store.tasks[0].status).toBe("done");
  });

  it("clearFinishedTasks 只保留 pending/running", async () => {
    enqueueTasks(["a.mp4", "b.mp4"], "en", "zh");
    store.tasks[0].status = "done";
    await clearFinishedTasks();
    expect(store.tasks).toHaveLength(1);
    expect(store.tasks[0].status).toBe("pending");
  });
});

describe("历史父子任务恢复", () => {
  it("恢复每个版本真实状态、输出目录和空间摘要", () => {
    restorePersistentTasks([
      {
        task_id: "p1",
        status: "PartiallyFailed",
        updated_at: 100,
        revision: 5,
        source_video: "C:/video.mp4",
        source_language: "en",
        targets: [
          { id: "zh-CN", language: "zh", dialect: "mandarin", display_name: "中文（普通话）", translate_style: "普通话", tts_accent: "普通话" },
          { id: "ja", language: "ja", display_name: "日语", translate_style: "", tts_accent: "" },
        ],
        shared_stages: [
          { stage: "media_probe", status: "Done" },
          { stage: "extract_audio", status: "Done" },
          { stage: "separation", status: "Skipped" },
          { stage: "stt", status: "Done" },
        ],
        shared_bytes: 1024,
        task_root: "C:/tasks/p1",
        variant_bytes: { "zh-CN": 2048, ja: 4096 },
        children: [
          { variant_id: "zh-CN", status: "Completed", output_dir: "C:/tasks/p1/targets/zh-CN", bytes: 2048 },
          { variant_id: "ja", status: "Failed", output_dir: "C:/tasks/p1/targets/ja", bytes: 4096 },
        ],
      },
    ]);
    expect(store.tasks).toHaveLength(1);
    expect(store.tasks[0].status).toBe("error");
    expect(store.tasks[0].children[0].status).toBe("done");
    expect(store.tasks[0].children[1].status).toBe("error");
    expect(store.tasks[0].children[0].outputDir).toContain("zh-CN");
    expect(store.tasks[0].sharedStages.every((stage) => stage.status === "done")).toBe(true);
    expect(store.tasks[0].logs[0]).toContain("共享产物");
  });
});

describe("步骤标签契约", () => {
  it("PIPELINE_STEPS 每一步都有中文标签", () => {
    for (const step of PIPELINE_STEPS) {
      expect(STEP_LABELS[step], `步骤 ${step} 缺少标签`).toBeTruthy();
    }
  });
});
