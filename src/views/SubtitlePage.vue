<template>
  <div>
    <PageHeader
      title="字幕编辑"
      :description="ed.taskId ? (ed.mode === 'source' ? '编辑父级 STT 原文；保存后所有目标翻译失效' : `编辑 ${ed.variantId} 译文；保存后仅该版本下游失效`) : '校对识别与翻译结果，导出 SRT 字幕文件'"
    >
      <template #actions>
        <n-button size="small" :disabled="!store.lastSegments.length" @click="loadFromTask">
          载入最近任务
        </n-button>
        <n-button size="small" @click="importSrt">导入 SRT</n-button>
        <n-button
          v-if="ed.taskId"
          size="small"
          type="primary"
          :disabled="!ed.segments.length || saving"
          :loading="saving"
          @click="saveToTask"
        >
          保存到任务
        </n-button>
        <n-button
          size="small"
          :disabled="!ed.segments.length"
          @click="exportSrt"
        >
          导出 SRT
        </n-button>
      </template>
    </PageHeader>

    <!-- 空状态 -->
    <div v-if="!ed.segments.length" class="vt-card empty-state">
      <Icon name="subtitle" :size="40" :stroke="1.2" class="empty-icon" />
      <p class="empty-title">暂无字幕内容</p>
      <p class="empty-desc">完成一次翻译任务后载入，或直接导入现有 SRT 文件进行校对</p>
      <div class="empty-actions">
        <n-button v-if="store.lastSegments.length" type="primary" secondary @click="loadFromTask">
          载入最近任务（{{ store.lastSegments.length }} 条）
        </n-button>
        <n-button secondary @click="importSrt">导入 SRT 文件</n-button>
      </div>
    </div>

    <template v-else>
      <!-- 工具条 -->
      <div class="toolbar">
        <span class="seg-count">共 {{ ed.segments.length }} 条</span>
        <label class="bilingual-toggle">
          <n-switch v-model:value="ed.bilingual" size="small" />
          <span>导出双语字幕</span>
        </label>
        <n-button size="tiny" text type="error" @click="clearAll">清空</n-button>
      </div>

      <!-- 编辑表 -->
      <div class="vt-card table-card">
        <table class="seg-table">
          <thead>
            <tr>
              <th class="col-idx">#</th>
              <th class="col-time">开始</th>
              <th class="col-time">结束</th>
              <th>原文</th>
              <th>译文</th>
              <th class="col-op"></th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="(seg, i) in ed.segments" :key="i">
              <td class="col-idx vt-mono">{{ i + 1 }}</td>
              <td class="col-time">
                <n-input-number
                  v-model:value="seg.start"
                  :min="0"
                  :step="0.1"
                  :precision="1"
                  size="small"
                  :show-button="false"
                />
              </td>
              <td class="col-time">
                <n-input-number
                  v-model:value="seg.end"
                  :min="0"
                  :step="0.1"
                  :precision="1"
                  size="small"
                  :show-button="false"
                />
              </td>
              <td>
                <n-input v-model:value="seg.text" size="small" type="textarea" :autosize="{ minRows: 1, maxRows: 3 }" />
              </td>
              <td>
                <n-input v-model:value="seg.translated" size="small" type="textarea" :autosize="{ minRows: 1, maxRows: 3 }" placeholder="未翻译" />
              </td>
              <td class="col-op">
                <button class="op-btn" title="删除此行" @click="ed.segments.splice(i, 1)">
                  <Icon name="trash" :size="14" />
                </button>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </template>
  </div>
</template>

<script setup lang="ts">
import { ref, watch, nextTick } from "vue";
import { NButton, NInput, NInputNumber, NSwitch, useMessage } from "naive-ui";
import PageHeader from "../components/PageHeader.vue";
import Icon from "../components/Icon.vue";
import { store } from "../lib/store";
import { buildSrt, parseSrt } from "../lib/srt";
import {
  loadTaskSegments,
  readTextFile,
  saveTaskSegments,
  writeTextFile,
} from "../lib/api";

const message = useMessage();

/** 字幕编辑会话状态（模块级，切页不丢失） */
const ed = store.subtitleEditor;
const saving = ref(false);

watch(
  () => [ed.taskId, ed.variantId, ed.mode] as const,
  async ([taskId, variantId]) => {
    if (!taskId) return;
    try {
      ed.segments = await loadTaskSegments(taskId, variantId ?? undefined);
      await nextTick();
      ed.dirty = false;
    } catch (e) {
      ed.segments = [];
      message.error(`载入任务字幕失败：${e}`);
    }
  },
  { immediate: true },
);

watch(
  () => ed.segments,
  () => {
    ed.dirty = true;
  },
  { deep: true },
);

function loadFromTask() {
  ed.segments = store.lastSegments.map((s) => ({ ...s }));
  message.success(`已载入 ${ed.segments.length} 条字幕`);
}

async function importSrt() {
  try {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const path = await open({
      multiple: false,
      filters: [{ name: "字幕文件", extensions: ["srt"] }],
      title: "导入 SRT 字幕",
    });
    if (!path) return;
    const content = await readTextFile(path as string);
    const parsed = parseSrt(content);
    if (!parsed.length) {
      message.warning("未能解析出字幕内容");
      return;
    }
    ed.segments = parsed;
    message.success(`已导入 ${parsed.length} 条字幕`);
  } catch (e) {
    message.error(`导入失败：${e}`);
  }
}

async function saveToTask() {
  if (!ed.taskId) return;
  saving.value = true;
  try {
    await saveTaskSegments(ed.taskId, ed.variantId ?? undefined, ed.segments);
    ed.dirty = false;
    message.success(
      ed.mode === "source"
        ? "原文已保存，所有目标翻译已标记待重跑"
        : "译文已保存，仅当前版本配音和视频已标记待重跑",
    );
  } catch (e) {
    message.error(`保存失败：${e}`);
  } finally {
    saving.value = false;
  }
}

async function exportSrt() {
  try {
    const { save } = await import("@tauri-apps/plugin-dialog");
    const path = await save({
      defaultPath: ed.bilingual ? "bilingual.srt" : "translated.srt",
      filters: [{ name: "字幕文件", extensions: ["srt"] }],
      title: "导出 SRT 字幕",
    });
    if (!path) return;
    await writeTextFile(path, buildSrt(ed.segments, ed.bilingual));
    message.success("导出成功");
  } catch (e) {
    message.error(`导出失败：${e}`);
  }
}

function clearAll() {
  ed.segments = [];
}
</script>

<style scoped>
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

.empty-actions {
  display: flex;
  gap: 10px;
}

.toolbar {
  display: flex;
  align-items: center;
  gap: 16px;
  margin-bottom: 12px;
}

.seg-count {
  font-size: 13px;
  color: var(--vt-text-2);
}

.bilingual-toggle {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  color: var(--vt-text-2);
  cursor: pointer;
}

.toolbar .n-button {
  margin-left: auto;
}

.table-card {
  padding: 0;
  overflow: hidden;
}

.seg-table {
  width: 100%;
  border-collapse: collapse;
}

.seg-table th {
  text-align: left;
  font-size: 12px;
  font-weight: 600;
  color: var(--vt-text-3);
  padding: 10px 12px;
  background: var(--vt-surface-sunken);
  border-bottom: 1px solid var(--vt-border);
  position: sticky;
  top: 0;
  z-index: 1;
}

.seg-table td {
  padding: 6px 12px;
  border-bottom: 1px solid var(--vt-border);
  vertical-align: middle;
  font-size: 13px;
}

.seg-table tr:last-child td {
  border-bottom: none;
}

.col-idx {
  width: 40px;
  color: var(--vt-text-3);
  font-size: 12px;
}

.col-time {
  width: 110px;
}

.col-op {
  width: 44px;
  text-align: center;
}

.op-btn {
  border: none;
  background: transparent;
  color: var(--vt-text-3);
  cursor: pointer;
  width: 26px;
  height: 26px;
  border-radius: 6px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  transition: background 0.15s, color 0.15s;
}

.op-btn:hover {
  background: var(--vt-error-weak);
  color: var(--vt-error);
}
</style>
