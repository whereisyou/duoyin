<template>
  <div>
    <PageHeader title="任务队列" description="管理所有翻译任务，串行执行">
      <template #actions>
        <n-button v-if="hasFinished" size="small" @click="onClearFinished">
          清空已结束
        </n-button>
      </template>
    </PageHeader>

    <!-- 统计条 -->
    <div class="stats-row">
      <div v-for="s in statItems" :key="s.label" class="stat-card">
        <span class="vt-dot" :class="s.dot"></span>
        <span class="stat-label">{{ s.label }}</span>
        <span class="stat-num vt-mono">{{ s.value }}</span>
      </div>
    </div>

    <!-- 空状态 -->
    <div v-if="store.tasks.length === 0" class="vt-card empty-state">
      <Icon name="list" :size="40" :stroke="1.2" class="empty-icon" />
      <p class="empty-title">暂无任务</p>
      <p class="empty-desc">前往工作台添加视频开始翻译</p>
      <n-button type="primary" secondary @click="navigateTo('home')">
        去添加任务
      </n-button>
    </div>

    <!-- 任务列表 -->
    <div v-else class="task-list">
      <div v-for="task in sortedTasks" :key="task.key" class="vt-card task-card">
        <div class="task-main">
          <div class="task-icon" :class="task.status">
            <Icon :name="statusIcon(task.status)" :size="17" />
          </div>

          <div class="task-info">
            <div class="task-name-row">
              <span class="task-name">{{ task.fileName }}</span>
              <n-tag size="small" :bordered="false" :type="statusTagType(task.status)">
                {{ statusLabel(task.status, task) }}
              </n-tag>
              <span class="task-langs vt-mono">{{ task.sourceLang }} → {{ task.targets.map((item) => item.display_name).join('、') }}</span>
            </div>
            <div class="task-path">{{ task.file }}</div>
            <div v-if="task.sharedBytes != null" class="space-summary">
              共享产物 {{ formatBytes(task.sharedBytes) }}
            </div>
            <div v-if="task.sharedStages.length" class="shared-stages">
              <span class="stage-caption">共享阶段</span>
              <n-tag
                v-for="stage in task.sharedStages"
                :key="stage.stage"
                size="tiny"
                :bordered="false"
                :type="statusTagType(stage.status)"
              >
                {{ STEP_LABELS[stage.stage] ?? stage.stage }} · {{ statusLabel(stage.status) }}
              </n-tag>
            </div>
            <div v-if="task.children.length > 1" class="child-summary">
              版本 {{ task.children.length }} · 运行 {{ childCount(task, 'running') }} · 完成 {{ childCount(task, 'done') }} · 失败 {{ childCount(task, 'error') }}
            </div>

            <!-- 运行中进度 -->
            <div v-if="task.status === 'running'" class="task-progress">
              <n-progress
                type="line"
                :percentage="task.progress"
                :height="6"
                border-radius="3px"
                :show-indicator="false"
              />
              <span class="task-step">{{ STEP_LABELS[task.step] ?? task.step }} {{ task.progress }}%</span>
            </div>
            <!-- 错误信息 -->
            <div v-else-if="task.status === 'error' && task.error" class="task-error">
              {{ task.error }}
            </div>

            <div v-if="task.children.length > 1 || task.children[0]?.variant.dialect" class="child-list">
              <div v-for="child in task.children" :key="child.variant.id" class="child-row">
                <span class="child-connector"></span>
                <span class="child-name">{{ child.variant.display_name }}</span>
                <n-tag size="tiny" :bordered="false" :type="statusTagType(child.status)">
                  {{ statusLabel(child.status) }}
                </n-tag>
                <span v-if="child.bytes != null" class="child-size">{{ formatBytes(child.bytes) }}</span>
                <span v-if="child.step && child.status === 'running'" class="child-step">
                  {{ STEP_LABELS[child.step] ?? child.step }} {{ child.progress }}%
                </span>
                <span v-if="child.error" class="child-error">{{ child.error }}</span>
                <button
                  v-if="task.status === 'running' && (child.status === 'pending' || child.status === 'running')"
                  class="act-btn child-open"
                  title="取消该版本"
                  @click="cancelChildTaskItem(task, child.variant.id)"
                >
                  <Icon name="x" :size="13" />
                </button>
                <button
                  v-if="task.backendId && task.status !== 'running'"
                  class="act-btn child-open"
                  title="编辑该版本译文"
                  @click="editTaskSubtitles(task, 'target', child.variant.id)"
                >
                  <Icon name="subtitle" :size="13" />
                </button>
                <button
                  v-if="task.backendId && task.status !== 'running'"
                  class="act-btn child-open"
                  title="导入该版本外部 SRT"
                  @click="handleImportSrt(task, child.variant.id)"
                >
                  <Icon name="subtitle" :size="13" />
                </button>
                <button
                  v-if="child.outputDir && child.status === 'done'"
                  class="act-btn child-open"
                  title="试听该版本配音"
                  @click="openDub(child.outputDir)"
                >
                  <Icon name="volume" :size="13" />
                </button>
                <button
                  v-if="child.outputDir"
                  class="act-btn child-open"
                  title="打开该版本目录"
                  @click="openPath(child.outputDir)"
                >
                  <Icon name="folder" :size="13" />
                </button>
              </div>
            </div>
          </div>

          <div class="task-side">
            <span v-if="taskDuration(task)" class="task-duration vt-mono">{{ taskDuration(task) }}</span>
            <div class="task-actions">
              <button
                v-if="task.backendId && task.status !== 'running'"
                class="act-btn"
                title="识别说话人并生成 speaker.json"
                @click="identifySpeakers(task)"
              >
                <Icon name="languages" :size="15" />
              </button>
              <button
                v-if="task.backendId"
                class="act-btn"
                title="查看产物明细"
                @click="toggleDetails(task)"
              >
                <Icon name="list" :size="15" />
              </button>
              <button
                v-if="task.backendId && task.status !== 'running'"
                class="act-btn"
                title="编辑父级 STT 原文"
                @click="editTaskSubtitles(task, 'source')"
              >
                <Icon name="subtitle" :size="15" />
              </button>
              <button
                v-if="task.status === 'done'"
                class="act-btn"
                title="打开输出目录"
                @click="openOutput(task)"
              >
                <Icon name="folder" :size="15" />
              </button>
              <button
                v-if="task.status === 'running' || task.status === 'pending'"
                class="act-btn"
                title="取消"
                @click="cancelTaskItem(task)"
              >
                <Icon name="x" :size="15" />
              </button>
              <button
                v-if="task.status === 'done' || task.status === 'error' || task.status === 'canceled'"
                class="act-btn"
                :title="task.status === 'done' ? '按现有编辑结果重新生成' : '重试'"
                @click="retryTaskItem(task)"
              >
                <Icon name="rotate-cw" :size="15" />
              </button>
              <button
                v-if="task.status !== 'running'"
                class="act-btn danger"
                title="删除"
                @click="onRemove(task)"
              >
                <Icon name="trash" :size="15" />
              </button>
            </div>
          </div>
        </div>
        <div v-if="task.artifactDetails" class="artifact-panel">
          <div v-for="artifact in task.artifactDetails" :key="artifact.id" class="artifact-row">
            <span>{{ artifact.kind }}</span>
            <span class="artifact-path vt-mono">{{ artifact.relative_path }}</span>
            <span>{{ formatBytes(artifact.size) }}</span>
            <n-tag size="tiny" :bordered="false">{{ artifact.status }}</n-tag>
          </div>
          <div v-if="task.artifactDetails.length === 0" class="task-path">暂无已提交产物</div>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from "vue";
import { NButton, NProgress, NTag, useMessage } from "naive-ui";
import PageHeader from "../components/PageHeader.vue";
import Icon from "../components/Icon.vue";
import {
  importTargetSrt,
  loadPersistentTask,
  openPath,
  runSpeakerDiarization,
} from "../lib/api";
import {
  store,
  taskStats,
  cancelTaskItem,
  cancelChildTaskItem,
  retryTaskItem,
  removeTaskItem,
  clearFinishedTasks,
  navigateTo,
  editTaskSubtitles,
  STEP_LABELS,
} from "../lib/store";
import type { TaskItem, TaskStatus } from "../lib/types";

const message = useMessage();

const statItems = computed(() => [
  { label: "全部", value: taskStats.value.total, dot: "idle" },
  { label: "排队中", value: taskStats.value.pending, dot: "warn" },
  { label: "进行中", value: taskStats.value.running, dot: "ok" },
  { label: "已完成", value: taskStats.value.done, dot: "ok" },
  { label: "失败", value: taskStats.value.error, dot: "err" },
]);

const hasFinished = computed(() =>
  store.tasks.some((t) => t.status === "done" || t.status === "error" || t.status === "canceled"),
);

/** 运行中置顶，其余按创建时间倒序 */
const sortedTasks = computed(() =>
  [...store.tasks].sort((a, b) => {
    const rank = (t: TaskItem) => (t.status === "running" ? 0 : t.status === "pending" ? 1 : 2);
    return rank(a) - rank(b) || b.createdAt - a.createdAt;
  }),
);

function statusIcon(s: TaskStatus): string {
  return { pending: "clock", running: "zap", done: "check", error: "alert-circle", canceled: "x" }[s];
}

function statusLabel(s: TaskStatus, task?: TaskItem): string {
  if (task?.parentStatus === "partially_failed") return "部分失败";
  return { pending: "排队中", running: "进行中", done: "已完成", error: "失败", canceled: "已取消" }[s];
}

function statusTagType(s: TaskStatus): "default" | "info" | "success" | "error" | "warning" {
  return ({ pending: "warning", running: "info", done: "success", error: "error", canceled: "default" } as const)[s];
}

function childCount(task: TaskItem, status: TaskStatus): number {
  return task.children.filter((child) => child.status === status).length;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

function taskDuration(task: TaskItem): string {
  if (!task.startedAt) return "";
  const end = task.finishedAt ?? Date.now();
  const sec = Math.round((end - task.startedAt) / 1000);
  if (sec < 60) return `${sec}s`;
  return `${Math.floor(sec / 60)}m${sec % 60}s`;
}


async function onRemove(task: TaskItem): Promise<void> {
  try {
    await removeTaskItem(task);
    message.success("已删除");
  } catch (e) {
    message.error(`删除失败：${String(e)}`);
  }
}

async function onClearFinished(): Promise<void> {
  try {
    await clearFinishedTasks();
    message.success("已清空");
  } catch (e) {
    message.error(`清空失败：${String(e)}`);
  }
}

async function identifySpeakers(task: TaskItem) {
  if (!task.backendId) return;
  try {
    const result = await runSpeakerDiarization(task.backendId);
    message.success(`识别出 ${new Set(result.map((item) => item.speaker)).size} 个说话人`);
    task.artifactDetails = undefined;
  } catch (e) {
    message.error(`说话人识别失败：${e}`);
  }
}

async function handleImportSrt(task: TaskItem, variantId: string) {
  if (!task.backendId) return;
  try {
    await importTargetSrt(task.backendId, variantId);
    message.success("SRT 已导入；重试任务时将跳过该版本翻译阶段");
    task.artifactDetails = undefined;
  } catch (e) {
    if (!String(e).includes("未选择")) message.error(`导入失败：${e}`);
  }
}

async function toggleDetails(task: TaskItem) {
  if (task.artifactDetails) {
    task.artifactDetails = undefined;
    return;
  }
  if (!task.backendId) return;
  try {
    const detail = await loadPersistentTask(task.backendId);
    task.artifactDetails = Object.entries(detail.manifest.artifacts).map(([id, artifact]) => ({
      id,
      ...artifact,
    }));
  } catch (e) {
    message.error(`读取任务详情失败：${e}`);
  }
}

async function openDub(outputDir: string) {
  const separator = outputDir.includes("\\") ? "\\" : "/";
  try {
    await openPath(`${outputDir}${separator}dub.wav`);
  } catch (e) {
    message.error(`试听失败：${e}`);
  }
}

async function openOutput(task: TaskItem) {
  // 完成后由后端返回实际输出目录（含未配置时的临时目录）
  if (!task.outputDir) {
    message.warning("输出目录未知，仅本次运行新完成的任务支持");
    return;
  }
  try {
    await openPath(task.outputDir);
  } catch (e) {
    message.error(`打开失败：${e}`);
  }
}
</script>

<style scoped>
.stats-row {
  display: flex;
  gap: 12px;
  margin-bottom: 16px;
}

.stat-card {
  flex: 1;
  background: var(--vt-surface);
  border: 1px solid var(--vt-border);
  border-radius: var(--vt-radius);
  padding: 12px 16px;
  display: flex;
  align-items: center;
  gap: 8px;
}

.stat-label {
  font-size: 13px;
  color: var(--vt-text-2);
}

.stat-num {
  margin-left: auto;
  font-size: 18px;
  font-weight: 700;
  color: var(--vt-text);
}

.empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 64px 24px;
  gap: 4px;
}

.empty-icon {
  color: var(--vt-text-3);
  margin-bottom: 8px;
}

.empty-title {
  font-size: 15px;
  font-weight: 600;
  margin: 0;
}

.empty-desc {
  font-size: 13px;
  color: var(--vt-text-3);
  margin: 0 0 16px;
}

.task-list {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.task-card {
  padding: 16px 20px;
}

.task-card + .task-card {
  margin-top: 0;
}

.artifact-panel {
  margin-top: 14px;
  padding-top: 12px;
  border-top: 1px solid var(--vt-border);
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.artifact-row {
  display: grid;
  grid-template-columns: 150px minmax(0, 1fr) 80px auto;
  align-items: center;
  gap: 10px;
  font-size: 12px;
  color: var(--vt-text-2);
}

.artifact-path {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--vt-text-3);
}

.task-main {
  display: flex;
  gap: 14px;
  align-items: flex-start;
}

.task-icon {
  width: 36px;
  height: 36px;
  border-radius: 9px;
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  background: var(--vt-bg);
  color: var(--vt-text-3);
}

.task-icon.running {
  background: var(--vt-accent-weak);
  color: var(--vt-accent);
}

.task-icon.done {
  background: var(--vt-success-weak);
  color: var(--vt-success);
}

.task-icon.error {
  background: var(--vt-error-weak);
  color: var(--vt-error);
}

.task-icon.pending {
  background: var(--vt-warning-weak);
  color: var(--vt-warning);
}

.task-info {
  flex: 1;
  min-width: 0;
}

.task-name-row {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}

.task-name {
  font-size: 14px;
  font-weight: 600;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.task-langs {
  font-size: 12px;
  color: var(--vt-text-3);
}

.child-list {
  margin-top: 12px;
  padding-left: 8px;
  border-left: 2px solid var(--vt-border);
  display: flex;
  flex-direction: column;
  gap: 7px;
}

.child-row {
  position: relative;
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 24px;
  font-size: 12px;
}

.child-connector {
  position: absolute;
  left: -8px;
  width: 8px;
  border-top: 2px solid var(--vt-border);
}

.child-name {
  min-width: 100px;
  color: var(--vt-text-2);
}

.child-step,
.child-size,
.space-summary {
  color: var(--vt-text-3);
}

.space-summary {
  margin-top: 6px;
  font-size: 12px;
}

.shared-stages,
.child-summary {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 8px;
  font-size: 12px;
  color: var(--vt-text-3);
}

.stage-caption {
  margin-right: 2px;
  color: var(--vt-text-2);
}

.child-error {
  color: var(--vt-error);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.child-open {
  margin-left: auto;
}

.task-path {
  font-size: 12px;
  color: var(--vt-text-3);
  margin-top: 2px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.task-progress {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-top: 10px;
  max-width: 520px;
}

.task-progress :deep(.n-progress) {
  flex: 1;
}

.task-step {
  font-size: 12px;
  color: var(--vt-text-2);
  white-space: nowrap;
}

.task-error {
  margin-top: 8px;
  font-size: 12.5px;
  color: var(--vt-error);
  background: var(--vt-error-weak);
  border-radius: 6px;
  padding: 6px 10px;
  user-select: text;
}

.task-side {
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 8px;
  flex-shrink: 0;
}

.task-duration {
  font-size: 12px;
  color: var(--vt-text-3);
}

.task-actions {
  display: flex;
  gap: 2px;
}

.act-btn {
  border: none;
  background: transparent;
  color: var(--vt-text-3);
  cursor: pointer;
  width: 28px;
  height: 28px;
  border-radius: 6px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  transition: background 0.15s, color 0.15s;
}

.act-btn:hover {
  background: var(--vt-bg);
  color: var(--vt-text);
}

.act-btn.danger:hover {
  background: var(--vt-error-weak);
  color: var(--vt-error);
}
</style>
