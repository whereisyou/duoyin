<template>
  <div>
    <PageHeader title="工作台" description="添加视频文件，配置语言，一键开始翻译">
      <template #actions>
        <n-button v-if="wb.stagedFiles.length" text @click="wb.stagedFiles = []">
          清空列表
        </n-button>
      </template>
    </PageHeader>

    <!-- 环境告警 -->
    <n-alert v-if="store.ffmpeg.status === 'missing'" type="error" :bordered="false" class="env-alert">
      FFmpeg 未找到，请安装并加入 PATH 后重启应用
    </n-alert>

    <!-- 1 · 选择视频 -->
    <section class="vt-card">
      <div class="vt-card-head">
        <span class="vt-step-no">1</span>
        <h2 class="vt-card-title">选择视频</h2>
        <p class="vt-card-desc">{{ wb.stagedFiles.length ? `已选择 ${wb.stagedFiles.length} 个文件` : '支持批量处理' }}</p>
      </div>

      <FileDrop @filesSelected="addFiles" />

      <!-- 待处理文件列表 -->
      <div v-if="wb.stagedFiles.length" class="file-list">
        <div v-for="(file, i) in wb.stagedFiles" :key="file" class="file-row">
          <Icon name="film" :size="16" class="file-icon" />
          <div class="file-info">
            <div class="file-name">{{ baseName(file) }}</div>
            <div class="file-path">{{ file }}</div>
          </div>
          <button class="icon-btn" title="移除" @click="wb.stagedFiles.splice(i, 1)">
            <Icon name="x" :size="14" />
          </button>
        </div>
      </div>
    </section>

    <!-- 2 · 翻译设置 -->
    <section class="vt-card">
      <div class="vt-card-head">
        <span class="vt-step-no">2</span>
        <h2 class="vt-card-title">翻译设置</h2>
      </div>

      <div class="form-grid">
        <div class="language-column">
          <label class="vt-field-label">源语言</label>
          <div class="language-field">
            <n-select
              v-model:value="wb.sourceLang"
              size="medium"
              :options="[...SOURCE_LANGS, ...LANGS]"
            />
          </div>
        </div>
        <div class="swap-cell">
          <Icon name="arrow-right" :size="15" />
        </div>
        <div class="language-column">
          <label class="vt-field-label">目标语言</label>
          <div class="language-field">
            <n-select
              v-model:value="wb.targetLangs"
              size="medium"
              multiple
              :options="LANGS.filter((o) => o.value !== wb.sourceLang)"
              :max-tag-count="2"
            />
            <n-button
              quaternary
              circle
              title="方言设置"
              :disabled="!wb.targetLangs.includes('zh')"
              @click="dialectModalOpen = true"
            >
              <template #icon><Icon name="settings" :size="15" /></template>
            </n-button>
          </div>
          <div class="variant-preview" :class="{ filled: selectedVariants.length }">
            <transition name="vt-fade">
              <span v-if="selectedVariants.length" class="variant-preview-text">
                将生成：{{ selectedVariants.map((item) => item.display_name).join('、') }}
              </span>
            </transition>
            <span v-if="!selectedVariants.length" class="variant-preview-text placeholder">
              选择目标语言后预览将要生成的版本
            </span>
          </div>
        </div>
      </div>

      <div class="advanced-entry">
        <n-button size="small" secondary class="advanced-btn" :class="{ glow: advancedGlow }" @click="openAdvancedSettings">
          <template #icon><Icon name="settings" :size="14" /></template>
          高级设置
        </n-button>
        <span v-if="!enhancementTags.length">背景分离（开启后保留原视频 BGM）、音轨、字幕和视频输出选项</span>
        <transition-group v-else name="vt-tag" tag="div" class="enhancement-tags">
          <n-tag
            v-for="tag in enhancementTags"
            :key="tag.key"
            size="small"
            round
            :type="tag.enhanced ? 'primary' : 'default'"
            :bordered="false"
          >
            {{ tag.label }}
          </n-tag>
        </transition-group>
      </div>

      <!-- 服务就绪状态 -->
      <div class="service-row">
        <div class="service-item">
          <span class="vt-dot" :class="sttReady ? 'ok' : 'warn'"></span>
          <span class="service-name">语音识别 · {{ sttEngine.label }}</span>
          <span class="service-state">{{ sttReady ? '已配置' : '未配置' }}</span>
        </div>
        <div class="service-item">
          <span class="vt-dot" :class="translateReady ? 'ok' : 'warn'"></span>
          <span class="service-name">字幕翻译 · {{ translateEngine.label }}</span>
          <span class="service-state">{{ translateReady ? '已配置' : '未配置' }}</span>
        </div>
        <div class="service-item">
          <span class="vt-dot" :class="ttsReady ? 'ok' : 'warn'"></span>
          <span class="service-name">配音合成 · {{ ttsEngine.label }}</span>
          <span class="service-state">{{ ttsStatusText }}</span>
        </div>
        <n-button v-if="!sttReady || !translateReady || !ttsReady" text type="primary" size="small" @click="goSettings">
          前往配置 →
        </n-button>
      </div>
    </section>

    <n-modal v-model:show="advancedModalOpen" preset="card" title="转换高级设置" class="dialect-modal">
      <n-space vertical size="large">
        <div class="advanced-group" :class="{ active: advancedDraft.separation_enabled }">
          <div class="advanced-title">背景音分离</div>
          <n-switch
            v-model:value="advancedDraft.separation_enabled"
            :disabled="!advancedDraft.separator_model_path"
          />
          <span class="dialect-help">{{ advancedDraft.separator_model_path ? '已配置 UVR-MDX 模型' : '请先选择 UVR-MDX ONNX 模型' }}</span>
          <n-alert v-if="!advancedDraft.separation_enabled" type="info" :bordered="false" style="margin-top: 4px; font-size: 12px;">
            未开启背景分离时，最终视频将只有新配音音轨，原视频背景音乐会丢失。如需保留 BGM，请开启此选项。
          </n-alert>
        </div>
        <div class="model-path-row">
          <n-input v-model:value="advancedDraft.separator_model_path" placeholder="UVR-MDX-NET-Inst_HQ_4.onnx" />
          <n-button :loading="downloadingUvr" @click="downloadSeparatorModel">自动下载</n-button>
          <n-button @click="chooseSeparatorModel">选择本地</n-button>
        </div>
        <n-checkbox
          v-model:checked="advancedDraft.separation_denoise"
          :disabled="!advancedDraft.separation_enabled"
        >
          分离后降噪（默认关闭）
        </n-checkbox>
        <n-checkbox
          v-model:checked="advancedDraft.separation_normalize"
          :disabled="!advancedDraft.separation_enabled"
        >
          分离后音量归一化（默认关闭）
        </n-checkbox>
        <n-checkbox v-model:checked="advancedDraft.separation_fallback_no_bgm">
          分离失败时退化为无背景音，仅使用新配音
        </n-checkbox>
        <n-checkbox v-model:checked="advancedDraft.generate_final_videos">输出最终视频</n-checkbox>
        <n-checkbox v-model:checked="advancedDraft.tts_use_video_prompt">
          配音克隆原视频音色（自动取原声作样本，ZipVoice 生效）
        </n-checkbox>
        <n-checkbox v-model:checked="advancedDraft.keep_original_audio_track">
          保留原音轨并添加新配音轨
        </n-checkbox>
        <div>
          <label class="vt-field-label">视频命名</label>
          <n-select v-model:value="advancedDraft.output_naming" :options="namingOptions" />
        </div>
        <div>
          <label class="vt-field-label">字幕输出</label>
          <n-select v-model:value="advancedDraft.subtitle_mode" :options="subtitleOptions" />
        </div>
        <div>
          <label class="vt-field-label">允许的配音变速范围</label>
          <div class="speed-row">
            <n-input-number v-model:value="advancedDraft.min_speed_percent" :min="50" :max="100" />
            <span>—</span>
            <n-input-number v-model:value="advancedDraft.max_speed_percent" :min="100" :max="200" />
            <span>%</span>
          </div>
        </div>
      </n-space>
      <template #footer>
        <div class="modal-actions">
          <n-button @click="advancedModalOpen = false">取消</n-button>
          <n-button type="primary" @click="saveAdvancedSettings">保存</n-button>
        </div>
      </template>
    </n-modal>

    <n-modal v-model:show="dialectModalOpen" preset="card" title="中文方言版本" class="dialect-modal">
      <p class="dialect-help">开启方言后，只生成你勾选的中文版本；普通话也是可选版本。</p>
      <n-checkbox-group v-model:value="selectedChineseDialects">
        <n-space vertical>
          <n-checkbox
            v-for="dialect in chineseDialects"
            :key="dialect.id"
            :value="dialect.id"
            :label="dialect.label"
          />
        </n-space>
      </n-checkbox-group>
      <template #footer>
        <div class="modal-actions">
          <n-button @click="resetChineseDialects">恢复普通话</n-button>
          <n-button type="primary" :disabled="selectedChineseDialects.length === 0" @click="saveChineseDialects">
            确定
          </n-button>
        </div>
      </template>
    </n-modal>

    <!-- 运行面板 -->
    <section v-if="currentTask || wb.queueJustFinished" class="vt-card run-panel">
      <div class="vt-card-head">
        <span class="vt-step-no">3</span>
        <h2 class="vt-card-title">运行状态</h2>
        <p class="vt-card-desc" v-if="currentTask">{{ currentTask.fileName }}</p>
      </div>

      <template v-if="currentTask">
        <!-- 步骤条 -->
        <div class="stepper">
          <template v-for="(step, i) in PIPELINE_STEPS" :key="step">
            <div class="stepper-item" :class="stepState(i)">
              <span class="stepper-dot">
                <Icon v-if="stepState(i) === 'done'" name="check" :size="11" :stroke="3" />
                <span v-else-if="stepState(i) === 'active'" class="stepper-pulse"></span>
              </span>
              <span class="stepper-label">{{ STEP_LABELS[step] }}</span>
            </div>
            <div v-if="i < PIPELINE_STEPS.length - 1" class="stepper-line" :class="{ filled: stepState(i) === 'done' }"></div>
          </template>
        </div>

        <!-- 总进度 -->
        <div class="progress-row">
          <n-progress
            type="line"
            :percentage="overallProgress"
            :height="8"
            border-radius="4px"
            :show-indicator="false"
          />
          <span class="progress-num vt-mono">{{ overallProgress }}%</span>
          <n-button size="small" quaternary @click="handleCancelCurrent">取消</n-button>
        </div>

        <!-- 日志 -->
        <div ref="logEl" class="log-box vt-mono">
          <div v-for="(line, i) in currentTask.logs" :key="i">{{ line }}</div>
          <div v-if="!currentTask.logs.length" class="log-empty">等待输出…</div>
        </div>
      </template>

      <div v-else class="finish-state">
        <Icon name="check" :size="20" class="finish-icon" />
        <span>队列已全部处理完成</span>
        <n-button size="small" type="primary" secondary @click="goTasks">查看任务队列</n-button>
      </div>
    </section>

    <!-- 底部吸底操作条 -->
    <div class="action-bar">
      <div class="action-left">
        <div class="action-summary">
          <template v-if="wb.stagedFiles.length">
            共 <b>{{ wb.stagedFiles.length }}</b> 个文件 · {{ langLabel(wb.sourceLang) }} → {{ selectedVariants.map((item) => item.display_name).join('、') }}
          </template>
          <template v-else>请先选择视频文件</template>
        </div>

        <div v-if="hasTasks" class="queue-mini" @click="goTasks">
          <div class="queue-mini-top">
            <span class="queue-title">任务队列</span>
            <span class="queue-counts">运行 {{ taskStats.running }} · 排队 {{ taskStats.pending }} · 完成 {{ taskStats.done }} · 错误 {{ taskStats.error }}</span>
          </div>
          <div v-if="currentTask" class="queue-current">
            <span class="queue-file">{{ currentTask.fileName }}</span>
            <span class="queue-step">{{ STEP_LABELS[currentTask.step] ?? currentTask.step }} · {{ overallProgress }}%</span>
          </div>
        </div>
      </div>
      <n-button
        type="primary"
        size="large"
        :disabled="!canStart"
        @click="handleStart"
      >
        <template #icon><Icon name="play" :size="15" /></template>
        {{ isQueueActive ? '追加到队列' : '开始翻译' }}
      </n-button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, watch, nextTick } from "vue";
import {
  NAlert,
  NButton,
  NSelect,
  NProgress,
  NModal,
  NCheckbox,
  NCheckboxGroup,
  NSpace,
  NSwitch,
  NInputNumber,
  NInput,
  NTag,
  useMessage,
} from "naive-ui";
import PageHeader from "../components/PageHeader.vue";
import FileDrop from "../components/FileDrop.vue";
import Icon from "../components/Icon.vue";
import {
  store,
  currentTask,
  taskStats,
  enqueueTasks,
  runQueue,
  cancelTaskItem,
  navigateTo,
  PIPELINE_STEPS,
  STEP_LABELS,
} from "../lib/store";
import {
  LANGS,
  SOURCE_LANGS,
  langLabel,
} from "../lib/langs";
import {
  expandTargetVariants,
  enabledEnhancementTags,
  normalizeAdvancedDraft,
  validateAdvancedDraft,
} from "../lib/variants";
import type { AppConfig, TargetVariant } from "../lib/types";
import { ensureUvrModel, getRuntimeInfo, pickOnnxModel, saveConfig } from "../lib/api";
import { STT_ENGINES, TRANSLATE_ENGINES, TTS_ENGINES, DEFAULT_CONFIG, engineById } from "../lib/engines";

const message = useMessage();

/** 工作台会话状态（模块级，切页不丢失） */
const wb = store.workbench;
const logEl = ref<HTMLElement | null>(null);
const dialectModalOpen = ref(false);
const advancedModalOpen = ref(false);
const downloadingUvr = ref(false);
const advancedDraft = ref<AppConfig>({ ...DEFAULT_CONFIG });
const namingOptions = [
  { label: "原文件名.版本.mp4（默认）", value: "source_variant" },
  { label: "final.mp4", value: "final" },
];
const subtitleOptions = [
  { label: "不烧字幕", value: "none" },
  { label: "输出外挂 SRT", value: "external_srt" },
  { label: "硬字幕（后续支持）", value: "hard_subtitle_planned", disabled: true },
];
const selectedChineseDialects = ref<string[]>([...(wb.dialectsByLanguage.zh ?? ["mandarin"])]);
const chineseDialects = computed(
  () => store.dialectSpecs.find((item) => item.language === "zh")?.dialects ?? [],
);
const selectedVariants = computed<TargetVariant[]>(() =>
  expandTargetVariants(wb.targetLangs, wb.dialectsByLanguage, store.dialectSpecs),
);
/** 已启用高阶功能的常驻标签（主界面可见，不打开弹窗也能知道开了什么） */
const enhancementTags = computed(() => enabledEnhancementTags(cfgOrDefault.value));
/** 高级设置保存后的扩散光晕动画开关 */
const advancedGlow = ref(false);

const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;

/** 引擎就绪状态（配置未加载时用默认值兜底） */
const cfgOrDefault = computed(() => store.config ?? DEFAULT_CONFIG);
const sttEngine = computed(() => engineById(STT_ENGINES, cfgOrDefault.value.stt_engine));
const translateEngine = computed(() => TRANSLATE_ENGINES[0]);
const ttsEngine = computed(() => engineById(TTS_ENGINES, cfgOrDefault.value.tts_engine));
const sttReady = computed(() => sttEngine.value.ready(cfgOrDefault.value));
const translateReady = computed(() => translateEngine.value.ready(cfgOrDefault.value));
const supertonicBaseReady = computed(
  () => store.runtime?.models.find((model) => model.id === "supertonic_base")?.ready ?? false,
);
const supertonicZhReady = computed(
  () => store.runtime?.models.find((model) => model.id === "supertonic_zh")?.ready ?? false,
);
const needsSupertonicZh = computed(
  () => cfgOrDefault.value.tts_engine === "supertonic" && selectedVariants.value.some((item) => item.language === "zh"),
);
const ttsReady = computed(() => {
  if (!ttsEngine.value.ready(cfgOrDefault.value)) return false;
  if (cfgOrDefault.value.tts_engine !== "supertonic") return true;
  return supertonicBaseReady.value && (!needsSupertonicZh.value || supertonicZhReady.value);
});
const ttsStatusText = computed(() => {
  if (cfgOrDefault.value.tts_engine === "supertonic" && !supertonicBaseReady.value) {
    return "Supertonic 基础模型不完整";
  }
  if (needsSupertonicZh.value && !supertonicZhReady.value) return "缺少 Supertonic-ZH 中文扩展";
  return ttsReady.value ? "已配置" : "未配置";
});
const isQueueActive = computed(() => !!currentTask.value || store.tasks.some((t) => t.status === "pending"));
const hasTasks = computed(() => store.tasks.length > 0);

const canStart = computed(
  () =>
    wb.stagedFiles.length > 0 &&
    store.ffmpeg.status === "ok" &&
    sttReady.value &&
    translateReady.value &&
    ttsReady.value &&
    selectedVariants.value.length > 0,
);

const currentStepIndex = computed(() => {
  if (!currentTask.value) return -1;
  return PIPELINE_STEPS.indexOf(currentTask.value.step);
});

function stepState(i: number): "done" | "active" | "pending" {
  const cur = currentStepIndex.value;
  if (cur < 0) return "pending";
  if (i < cur) return "done";
  if (i === cur) return "active";
  return "pending";
}

/** 总进度按真实工作单元汇总：共享阶段各一次，目标阶段按版本分别计数。 */
const overallProgress = computed(() => currentTask.value?.progress ?? 0);

function addFiles(paths: string[]) {
  const videoExt = /\.(mp4|mkv|avi|mov|wmv|flv)$/i;
  const valid = paths.filter((p) => videoExt.test(p));
  const skipped = paths.length - valid.length;
  const before = wb.stagedFiles.length;
  wb.stagedFiles = [...new Set([...wb.stagedFiles, ...valid])];
  const added = wb.stagedFiles.length - before;
  if (skipped > 0) message.warning(`已跳过 ${skipped} 个非视频文件`);
  if (added === 0 && valid.length > 0) message.info("文件已在列表中");
}

watch(
  () => wb.sourceLang,
  (src) => {
    if (src !== "auto" && wb.targetLangs.includes(src)) {
      wb.targetLangs = wb.targetLangs.filter((item) => item !== src);
    }
  },
);

async function downloadSeparatorModel() {
  downloadingUvr.value = true;
  try {
    advancedDraft.value.separator_model_path = await ensureUvrModel();
    message.success("UVR-MDX 模型已就绪");
  } catch (e) {
    message.error(`模型下载失败：${e}`);
  } finally {
    downloadingUvr.value = false;
  }
}

async function chooseSeparatorModel() {
  const path = await pickOnnxModel();
  if (path) advancedDraft.value.separator_model_path = path;
}

function openAdvancedSettings() {
  advancedDraft.value = { ...(store.config ?? DEFAULT_CONFIG) };
  advancedModalOpen.value = true;
}

async function saveAdvancedSettings() {
  const error = validateAdvancedDraft(advancedDraft.value);
  if (error) {
    message.error(error);
    return;
  }
  const normalized = normalizeAdvancedDraft(advancedDraft.value);
  await saveConfig(normalized);
  store.config = { ...normalized };
  try {
    store.runtime = await getRuntimeInfo();
  } catch {
    store.runtime = null;
  }
  advancedModalOpen.value = false;
  message.success("高级设置已保存");
  if (enhancementTags.value.length) {
    advancedGlow.value = true;
    setTimeout(() => (advancedGlow.value = false), 1500);
  }
}

function resetChineseDialects() {
  selectedChineseDialects.value = ["mandarin"];
}

function saveChineseDialects() {
  wb.dialectsByLanguage.zh = [...selectedChineseDialects.value];
  dialectModalOpen.value = false;
}

function handleStart() {
  const n = enqueueTasks(wb.stagedFiles, wb.sourceLang, selectedVariants.value);
  if (n === 0) {
    message.info("这些文件已在队列中");
    return;
  }
  wb.queueJustFinished = false;
  wb.stagedFiles = [];
  message.success(`已添加 ${n} 个任务`);
  void runQueue();
}

async function handleCancelCurrent() {
  if (currentTask.value) {
    await cancelTaskItem(currentTask.value);
  }
}

function goSettings() {
  navigateTo("settings");
}

function goTasks() {
  navigateTo("tasks");
}

// 队列从运行变为空闲 → 显示完成态
watch(isQueueActive, (active, prev) => {
  if (prev && !active && store.tasks.length > 0) {
    wb.queueJustFinished = true;
  }
});

// 日志自动滚动到底部
watch(
  () => currentTask.value?.logs.length,
  async () => {
    await nextTick();
    if (logEl.value) logEl.value.scrollTop = logEl.value.scrollHeight;
  },
);
</script>

<style scoped>
.env-alert {
  margin-bottom: 16px;
}

.advanced-entry {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 10px;
  margin-top: 12px;
  font-size: 12px;
  color: var(--vt-text-3);
}

/* 高阶功能保存后：按钮向外扩散两圈主题色光晕（克制、1.5s 内结束） */
.advanced-btn.glow {
  animation: vt-ripple 0.65s ease-out 2;
}

@keyframes vt-ripple {
  0% {
    box-shadow: 0 0 0 0 rgba(99, 152, 255, 0.4);
  }
  100% {
    box-shadow: 0 0 0 14px rgba(99, 152, 255, 0);
  }
}

.enhancement-tags {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.vt-tag-enter-active {
  transition: opacity 0.3s ease, transform 0.3s ease;
}

.vt-tag-enter-from {
  opacity: 0;
  transform: scale(0.7);
}

.vt-tag-leave-active {
  transition: opacity 0.2s ease;
}

.vt-tag-leave-to {
  opacity: 0;
}

/* 弹窗内：开关激活时分组左缘主题色提示 + 背景微色过渡 */
.advanced-group {
  padding: 8px 10px;
  border-left: 3px solid transparent;
  border-radius: 4px;
  transition: border-color 0.3s ease, background-color 0.3s ease;
}

.advanced-group.active {
  border-left-color: rgba(99, 152, 255, 0.75);
  background-color: rgba(99, 152, 255, 0.06);
}

.model-path-row {
  display: flex;
  gap: 8px;
}

.model-path-row :deep(.n-input) {
  flex: 1;
}

.advanced-group {
  display: flex;
  align-items: center;
  gap: 10px;
}

.advanced-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--vt-text);
}

.speed-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.speed-row :deep(.n-input-number) {
  width: 110px;
}

.language-column {
  min-width: 0;
}

.language-field {
  display: flex;
  align-items: flex-start;
  height: 34px;
  gap: 6px;
}

.language-field :deep(.n-select) {
  flex: 1;
  min-width: 0;
  height: 34px;
}

.language-field :deep(.n-base-selection) {
  height: 34px !important;
  min-height: 34px !important;
}

.language-field :deep(.n-button) {
  width: 34px;
  height: 34px;
  flex: 0 0 34px;
}

.variant-preview,
.dialect-help {
  margin-top: 7px;
  font-size: 12px;
  color: var(--vt-text-3);
}

/* 常驻占位：预览行永远保留高度，选中后内容淡入，不再推挤下方表单 */
.variant-preview {
  min-height: 19px;
  line-height: 19px;
}

.variant-preview-text {
  display: inline-block;
  max-width: 100%;
  vertical-align: bottom;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.variant-preview-text.placeholder {
  opacity: 0.55;
}

.vt-fade-enter-active {
  transition: opacity 0.35s ease, transform 0.35s ease;
}

.vt-fade-enter-from {
  opacity: 0;
  transform: translateY(3px);
}

.vt-fade-leave-active {
  display: none;
}

.dialect-modal {
  width: min(460px, calc(100vw - 32px));
}

.modal-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

.file-list {
  margin-top: 12px;
  border: 1px solid var(--vt-border);
  border-radius: var(--vt-radius-sm);
  overflow: hidden;
}

.file-row {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 14px;
  background: var(--vt-surface);
}

.file-row + .file-row {
  border-top: 1px solid var(--vt-border);
}

.file-icon {
  color: var(--vt-accent);
  flex-shrink: 0;
}

.file-info {
  min-width: 0;
  flex: 1;
}

.file-name {
  font-size: 13.5px;
  font-weight: 500;
  color: var(--vt-text);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.file-path {
  font-size: 12px;
  color: var(--vt-text-3);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.icon-btn {
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
  flex-shrink: 0;
}

.icon-btn:hover {
  background: var(--vt-error-weak);
  color: var(--vt-error);
}

.form-grid {
  display: grid;
  grid-template-columns: 1fr auto 1fr;
  gap: 12px;
  align-items: start;
}

/* 窄屏：语言区改单列，隐藏交换箭头 */
@media (max-width: 720px) {
  .form-grid {
    grid-template-columns: 1fr;
  }
  .swap-cell {
    display: none;
  }
}

.swap-cell {
  display: flex;
  align-items: center;
  height: 34px;
  margin-top: 25px;
}

.swap-btn:hover {
  background: var(--vt-accent-weak);
  color: var(--vt-accent);
}

.service-row {
  margin-top: 16px;
  padding-top: 14px;
  border-top: 1px dashed var(--vt-border);
  display: flex;
  align-items: center;
  gap: 24px;
  flex-wrap: wrap;
}

.service-item {
  display: flex;
  align-items: center;
  gap: 8px;
}

.service-name {
  font-size: 13px;
  color: var(--vt-text-2);
}

.service-state {
  font-size: 12px;
  color: var(--vt-text-3);
}

/* 运行面板 */
/* 9 步一行在窄窗/选多语言时必然溢出卡片右缘（"生成字幕/合成视频脱框"的根因）：
   改为可换行，行内虚线限宽保持每行整齐 */
.stepper {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  row-gap: 10px;
  margin-bottom: 18px;
}

.stepper-item {
  display: flex;
  align-items: center;
  gap: 7px;
  flex-shrink: 0;
}

.stepper-dot {
  width: 20px;
  height: 20px;
  border-radius: 50%;
  border: 2px solid var(--vt-border-strong);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  background: var(--vt-surface);
  transition: all 0.2s;
}

.stepper-item.done .stepper-dot {
  background: var(--vt-accent);
  border-color: var(--vt-accent);
  color: #fff;
}

.stepper-item.active .stepper-dot {
  border-color: var(--vt-accent);
}

.stepper-pulse {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--vt-accent);
  animation: pulse 1.2s ease-in-out infinite;
}

@keyframes pulse {
  0%, 100% { opacity: 1; transform: scale(1); }
  50% { opacity: 0.4; transform: scale(0.75); }
}

.stepper-label {
  font-size: 12.5px;
  color: var(--vt-text-3);
  white-space: nowrap;
}

.stepper-item.active .stepper-label {
  color: var(--vt-accent);
  font-weight: 600;
}

.stepper-item.done .stepper-label {
  color: var(--vt-text-2);
}

.stepper-line {
  flex: 1 1 12px;
  max-width: 28px;
  height: 2px;
  background: var(--vt-border);
  margin: 0 10px;
  min-width: 12px;
}

.stepper-line.filled {
  background: var(--vt-accent);
}

.progress-row {
  display: flex;
  align-items: center;
  gap: 12px;
}

.progress-row :deep(.n-progress) {
  flex: 1;
}

.progress-num {
  font-size: 13px;
  font-weight: 600;
  color: var(--vt-text-2);
  min-width: 42px;
  text-align: right;
}

.log-box {
  margin-top: 14px;
  height: 120px;
  overflow-y: auto;
  background: var(--vt-surface-sunken);
  border: 1px solid var(--vt-border);
  border-radius: var(--vt-radius-sm);
  padding: 10px 14px;
  font-size: 12px;
  line-height: 1.8;
  color: var(--vt-text-2);
  user-select: text;
}

.log-empty {
  color: var(--vt-text-3);
}

.finish-state {
  display: flex;
  align-items: center;
  gap: 10px;
  color: var(--vt-text-2);
  font-size: 14px;
}

.finish-icon {
  color: var(--vt-success);
}

/* 底部吸底操作条 */
.action-bar {
  position: sticky;
  bottom: 0;
  margin: 20px -32px -32px;
  padding: 14px 32px;
  background: rgba(255, 255, 255, 0.9);
  backdrop-filter: blur(8px);
  border-top: 1px solid var(--vt-border);
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  z-index: 10;
}

.action-left {
  display: flex;
  align-items: center;
  gap: 16px;
  min-width: 0;
  flex: 1;
}

.action-summary {
  font-size: 13.5px;
  color: var(--vt-text-2);
  min-width: 0;
  flex-shrink: 1;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.queue-mini {
  min-width: min(260px, 38vw);
  max-width: 420px;
  padding: 8px 12px;
  border: 1px solid var(--vt-border);
  border-radius: var(--vt-radius-sm);
  background: var(--vt-surface);
  cursor: pointer;
  transition: border-color 0.15s, background 0.15s;
}

.queue-mini:hover {
  border-color: var(--vt-accent);
  background: var(--vt-accent-weak);
}

.queue-mini-top,
.queue-current {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}

.queue-title {
  font-size: 12.5px;
  font-weight: 600;
  color: var(--vt-text);
}

.queue-counts,
.queue-step {
  font-size: 12px;
  color: var(--vt-text-3);
  white-space: nowrap;
}

.queue-file {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 12.5px;
  color: var(--vt-text-2);
}
</style>
