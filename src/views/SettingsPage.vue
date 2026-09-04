<template>
  <div>
    <PageHeader title="设置" description="配置识别、翻译与合成服务">
      <template #actions>
        <n-button v-if="store.previousPage" text @click="goBack">
          <template #icon><Icon name="arrow-left" :size="15" /></template>
          返回{{ PAGE_LABELS[store.previousPage] }}
        </n-button>
      </template>
    </PageHeader>

    <div v-if="settings.draft" class="settings-layout">
      <!-- 左侧锚点导航 -->
      <nav class="anchor-nav">
        <div
          v-for="g in groups"
          :key="g.id"
          class="anchor-item"
          :class="{ active: activeSection === g.id }"
          @click="scrollTo(g.id)"
        >
          <Icon :name="g.icon" :size="15" />
          <span>{{ g.title }}</span>
          <span class="vt-dot" :class="g.ready(cfg) ? 'ok' : 'idle'"></span>
        </div>
      </nav>

      <!-- 配置分组（数据驱动渲染） -->
      <div class="settings-main">
        <section class="vt-card runtime-card">
          <div class="vt-card-head">
            <Icon name="cpu" :size="16" class="sec-icon" />
            <h2 class="vt-card-title">运行环境与模型</h2>
            <n-button size="tiny" text @click="refreshRuntime">刷新</n-button>
          </div>
          <div class="runtime-gpu">GPU：{{ runtime?.gpu || '未检测到 NVIDIA GPU，将使用 CPU' }}</div>
          <div class="runtime-models">
            <div v-for="model in runtime?.models ?? []" :key="model.id" class="runtime-model">
              <span class="vt-dot" :class="model.ready ? 'ok' : 'warn'"></span>
              <span>{{ model.id }}</span>
              <span class="runtime-path vt-mono">{{ model.path || '未配置' }}</span>
              <span>{{ formatBytes(model.bytes) }}</span>
            </div>
          </div>
        </section>
        <section v-for="g in groups" :key="g.id" :id="g.id" class="vt-card">
          <div class="vt-card-head">
            <Icon :name="g.icon" :size="16" class="sec-icon" />
            <h2 class="vt-card-title">{{ g.title }}</h2>
          </div>

          <!-- 引擎选择（STT / TTS） -->
          <div v-if="g.engineKey" class="field">
            <label class="vt-field-label">引擎</label>
            <n-select
              :value="cfg[g.engineKey]"
              :options="engineOptions(g.engines!)"
              @update:value="(v: string) => setEngine(g.engineKey!, v)"
            />
            <p v-if="currentEngine(g).desc" class="field-hint">{{ currentEngine(g).desc }}</p>
          </div>

          <ConfigFields :fields="currentFields(g)" :cfg="cfg" class="group-fields" />
        </section>

        <!-- 吸底保存条 -->
        <div class="save-bar">
          <span v-if="settings.dirty" class="dirty-hint">有未保存的修改</span>
          <n-button quaternary @click="openLogs">
            <template #icon><Icon name="file" :size="15" /></template>
            日志
          </n-button>
          <n-button @click="reload">还原</n-button>
          <n-button type="primary" :loading="saving" @click="save">保存配置</n-button>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, watch, onMounted, nextTick } from "vue";
import { NButton, NSelect, useMessage } from "naive-ui";
import PageHeader from "../components/PageHeader.vue";
import Icon from "../components/Icon.vue";
import ConfigFields from "../components/ConfigFields.vue";
import {
  getRuntimeInfo,
  loadConfig,
  saveConfig,
  getLogDir,
  openPath,
} from "../lib/api";
import type { RuntimeInfo } from "../lib/api";
import { store, goBack, PAGE_LABELS } from "../lib/store";
import {
  STT_ENGINES,
  TRANSLATE_ENGINES,
  TTS_ENGINES,
  DEFAULT_CONFIG,
  engineById,
} from "../lib/engines";
import type { EngineDef, FieldDef } from "../lib/engines";
import type { AppConfig } from "../lib/types";

type EngineKey = "stt_engine" | "tts_engine";

interface GroupDef {
  id: string;
  title: string;
  icon: string;
  /** 有值则渲染引擎下拉；无值则直接使用 fields 或唯一引擎 */
  engineKey?: EngineKey;
  engines?: EngineDef[];
  fields?: FieldDef[];
  ready: (cfg: AppConfig) => boolean;
}

const message = useMessage();

/** 设置草稿（模块级，切页保留未保存修改）；模板在 draft 就绪后才渲染 */
const settings = store.settings;
const cfg = computed<AppConfig>(() => settings.draft ?? DEFAULT_CONFIG);
const saving = ref(false);
const runtime = ref<RuntimeInfo | null>(null);
const activeSection = ref("sec-stt");

/** 初始化/还原期间不标脏 */
let initializing = false;

const groups: GroupDef[] = [
  {
    id: "sec-stt",
    title: "语音识别（STT）",
    icon: "mic",
    engineKey: "stt_engine",
    engines: STT_ENGINES,
    ready: (c) => engineById(STT_ENGINES, c.stt_engine).ready(c),
  },
  {
    id: "sec-translate",
    title: "字幕翻译",
    icon: "languages",
    engines: TRANSLATE_ENGINES,
    ready: (c) => TRANSLATE_ENGINES[0].ready(c),
  },
  {
    id: "sec-api",
    title: "外部 API 调度",
    icon: "globe",
    fields: [
      {
        key: "http_proxy",
        label: "HTTP/HTTPS 代理",
        type: "text",
        placeholder: "例如 http://127.0.0.1:7890，留空使用系统设置",
        hint: "用于远程翻译 API；localhost 模型服务不会使用该代理。",
      },
      {
        key: "api_max_concurrent",
        label: "最大并发请求数",
        type: "number",
        placeholder: "1",
        hint: "所有外部 API 请求共用。限流严格的服务建议 1；多账号/高配额可调大。",
      },
      {
        key: "api_interval_ms",
        label: "请求启动间隔（毫秒）",
        type: "number",
        placeholder: "1000",
        hint: "两次 API 请求开始之间至少间隔这么久，避免触发 QPS 限流。",
      },
    ],
    ready: (c) => c.api_max_concurrent > 0,
  },
  {
    id: "sec-tts",
    title: "语音合成（TTS）",
    icon: "volume",
    engineKey: "tts_engine",
    engines: TTS_ENGINES,
    ready: (c) => engineById(TTS_ENGINES, c.tts_engine).ready(c),
  },
  {
    id: "sec-diarization",
    title: "说话人识别",
    icon: "languages",
    fields: [
      { key: "diarization_seg_model", label: "Pyannote 分割模型", type: "text", browse: "file", placeholder: "seg_model.onnx" },
      { key: "diarization_embedding_model", label: "说话人嵌入模型", type: "text", browse: "file", placeholder: "eres2net / titanet onnx" },
      { key: "diarization_num_speakers", label: "已知说话人数", type: "number", placeholder: "-1 自动检测", hint: "未知填 -1；已知人数可提高聚类稳定性" },
    ],
    ready: (c) => !!c.diarization_seg_model && !!c.diarization_embedding_model,
  },

];

const engineOptions = (engines: EngineDef[]) =>
  engines.map((e) => ({ label: e.label, value: e.id }));

const currentEngine = (g: GroupDef): EngineDef =>
  engineById(g.engines ?? [], g.engineKey ? String(cfg.value[g.engineKey]) : "");

const currentFields = (g: GroupDef): FieldDef[] => g.fields ?? currentEngine(g).fields;

function setEngine(key: EngineKey, id: string) {
  if (settings.draft) settings.draft[key] = id;
}

function scrollTo(id: string) {
  activeSection.value = id;
  document.getElementById(id)?.scrollIntoView({ behavior: "smooth", block: "start" });
}

/** 首次进入时初始化草稿：优先用内存中的配置，否则从后端读 */
function formatBytes(bytes: number): string {
  if (!bytes) return "";
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

async function refreshRuntime() {
  try {
    runtime.value = await getRuntimeInfo();
    store.runtime = runtime.value;
  } catch (e) {
    message.error(`检测运行环境失败：${e}`);
  }
}

async function ensureDraft() {
  if (settings.draft) return;
  initializing = true;
  let base = store.config;
  if (!base) {
    try {
      base = await loadConfig();
      store.config = base;
    } catch {
      base = null;
    }
  }
  settings.draft = { ...DEFAULT_CONFIG, ...(base ?? {}) };
  await nextTick();
  initializing = false;
}

/** 还原：从后端（磁盘）重新读取，放弃未保存修改 */
async function reload() {
  initializing = true;
  try {
    const loaded = await loadConfig();
    settings.draft = { ...DEFAULT_CONFIG, ...loaded };
    store.config = { ...settings.draft };
    await nextTick();
    settings.dirty = false;
  } catch (e) {
    message.error(`读取配置失败：${e}`);
  } finally {
    initializing = false;
  }
}

/** 打开后端日志目录（排查崩溃/异常时先看这里） */
async function openLogs() {
  try {
    await openPath(await getLogDir());
  } catch (e) {
    message.error(`打开日志目录失败：${e}`);
  }
}

/** 保存：写磁盘 + 更新后端内存状态 + 同步前端全局配置 */
async function save() {
  if (!settings.draft) return;
  saving.value = true;
  try {
    // 兑底：所有字符串字段去首尾空格（URL / Key / 路径复制粘贴易带空白）
    const draft = { ...settings.draft } as Record<string, unknown>;
    for (const k of Object.keys(draft)) {
      if (typeof draft[k] === "string") draft[k] = (draft[k] as string).trim();
    }
    settings.draft = draft as unknown as AppConfig;
    await saveConfig({ ...settings.draft });
    store.config = { ...settings.draft };
    await refreshRuntime();
    settings.dirty = false;
    message.success(store.previousPage ? "配置已保存，可返回继续操作" : "配置已保存");
  } catch (e) {
    message.error(`保存失败：${e}`);
  } finally {
    saving.value = false;
  }
}

onMounted(() => {
  void ensureDraft();
  void refreshRuntime();
});

// 输入即标记脏
watch(
  () => settings.draft,
  () => {
    if (!initializing) settings.dirty = true;
  },
  { deep: true },
);
</script>

<style scoped>
.runtime-card {
  margin-bottom: 16px;
}

.runtime-gpu {
  margin-bottom: 10px;
  font-size: 13px;
  color: var(--vt-text-2);
}

.runtime-models {
  display: flex;
  flex-direction: column;
  gap: 7px;
}

.runtime-model {
  display: grid;
  grid-template-columns: 10px 120px minmax(0, 1fr) 80px;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  color: var(--vt-text-2);
}

.runtime-path {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--vt-text-3);
}

.settings-layout {
  display: flex;
  gap: 20px;
  align-items: flex-start;
}

.anchor-nav {
  width: 168px;
  flex-shrink: 0;
  position: sticky;
  top: 32px;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.anchor-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  border-radius: var(--vt-radius-sm);
  font-size: 13px;
  color: var(--vt-text-2);
  cursor: pointer;
  transition: background 0.15s, color 0.15s;
}

.anchor-item .vt-dot {
  margin-left: auto;
}

.anchor-item:hover {
  background: var(--vt-surface);
}

.anchor-item.active {
  background: var(--vt-surface);
  color: var(--vt-accent);
  font-weight: 600;
  box-shadow: inset 0 0 0 1px var(--vt-border);
}

.settings-main {
  flex: 1;
  min-width: 0;
}

.sec-icon {
  color: var(--vt-accent);
}

.field {
  margin-bottom: 4px;
}

.group-fields {
  margin-top: 16px;
}

.field-hint {
  margin: 6px 0 0;
  font-size: 12px;
  color: var(--vt-text-3);
}

.save-bar {
  position: sticky;
  bottom: 0;
  margin: 20px 0 -32px;
  padding: 14px 0;
  background: linear-gradient(to top, var(--vt-bg) 70%, transparent);
  display: flex;
  justify-content: flex-end;
  align-items: center;
  gap: 10px;
  z-index: 10;
}

.dirty-hint {
  font-size: 12.5px;
  color: var(--vt-warning);
  margin-right: auto;
}
</style>
