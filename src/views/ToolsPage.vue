<template>
  <div>
    <PageHeader title="媒体工具" description="常用 FFmpeg 媒体处理，不影响视频翻译任务" />

    <section class="vt-card tool-card">
      <h2 class="vt-card-title">ASS 字幕样式与预览</h2>
      <div class="ass-grid">
        <n-input v-model:value="ass.fontName" placeholder="字体，例如 Microsoft YaHei" />
        <n-input-number v-model:value="ass.fontSize" :min="12" :max="120" />
        <n-color-picker v-model:value="ass.primaryColor" />
        <n-color-picker v-model:value="ass.outlineColor" />
      </div>
      <div class="ass-preview" :style="assPreviewStyle">字幕样式预览 Subtitle Preview</div>
      <div class="time-row">
        <n-button @click="loadAssSrt">载入 SRT</n-button>
        <n-button type="primary" :disabled="ass.segments.length === 0" @click="exportAss">导出 ASS</n-button>
        <span>已载入 {{ ass.segments.length }} 条</span>
      </div>
    </section>

    <section class="vt-card tool-card">
      <h2 class="vt-card-title">实时语音识别</h2>
      <div class="time-row">
        <n-select v-model:value="realtime.language" :options="[...SOURCE_LANGS, ...LANGS]" />
        <n-button v-if="!recording" type="primary" @click="startRealtime">开始录音</n-button>
        <n-button v-else type="error" @click="stopRealtime">停止录音</n-button>
      </div>
      <n-input v-model:value="realtime.text" type="textarea" :autosize="{ minRows: 4, maxRows: 10 }" placeholder="识别文本会追加到这里" />
    </section>

    <section class="vt-card tool-card">
      <h2 class="vt-card-title">字幕与文稿匹配</h2>
      <PathPicker v-model="match.srt" label="时间轴 SRT" mode="file" />
      <PathPicker v-model="match.text" label="目标文稿 TXT" mode="file" />
      <PathPicker v-model="match.output" label="输出 SRT" mode="save" extension="srt" />
      <n-button type="primary" :loading="busy === 'match'" @click="runMatch">开始匹配</n-button>
    </section>

    <section class="vt-card tool-card">
      <h2 class="vt-card-title">视频裁剪</h2>
      <PathPicker v-model="clip.input" label="输入视频" mode="file" />
      <div class="time-row">
        <n-input-number v-model:value="clip.start" :min="0" :step="0.1" />
        <span>至</span>
        <n-input-number v-model:value="clip.end" :min="0.1" :step="0.1" />
        <span>秒</span>
      </div>
      <PathPicker v-model="clip.output" label="输出视频" mode="save" extension="mp4" />
      <n-button type="primary" :loading="busy === 'clip'" @click="runClip">开始裁剪</n-button>
    </section>

    <section class="vt-card tool-card">
      <h2 class="vt-card-title">音视频分离</h2>
      <PathPicker v-model="demux.input" label="输入视频" mode="file" />
      <PathPicker v-model="demux.outputDir" label="输出目录" mode="dir" />
      <n-button type="primary" :loading="busy === 'demux'" @click="runDemux">开始分离</n-button>
    </section>

    <section class="vt-card tool-card">
      <h2 class="vt-card-title">视频与音频合并</h2>
      <PathPicker v-model="merge.video" label="视频文件" mode="file" />
      <PathPicker v-model="merge.audio" label="音频文件" mode="file" />
      <PathPicker v-model="merge.output" label="输出视频" mode="save" extension="mp4" />
      <n-button type="primary" :loading="busy === 'merge'" @click="runMerge">开始合并</n-button>
    </section>
  </div>
</template>

<script setup lang="ts">
import { computed, reactive, ref } from "vue";
import {
  NButton,
  NColorPicker,
  NInput,
  NInputNumber,
  NSelect,
  useMessage,
} from "naive-ui";
import PageHeader from "../components/PageHeader.vue";
import PathPicker from "../components/PathPicker.vue";
import {
  clipVideo,
  matchTextToSrt,
  mergeVideoAudio,
  separateMedia,
  transcribeAudioChunk,
} from "../lib/api";
import { LANGS, SOURCE_LANGS } from "../lib/langs";
import { buildAss } from "../lib/ass";
import { parseSrt } from "../lib/srt";
import type { SubtitleSegment } from "../lib/types";
import { readTextFile, writeTextFile } from "../lib/api";

const message = useMessage();
const busy = ref<"match" | "clip" | "demux" | "merge" | null>(null);
const match = reactive({ srt: "", text: "", output: "" });
const ass = reactive({
  fontName: "Microsoft YaHei",
  fontSize: 48,
  primaryColor: "#FFFFFF",
  outlineColor: "#000000",
  segments: [] as SubtitleSegment[],
});
const assPreviewStyle = computed(() => ({
  fontFamily: ass.fontName,
  fontSize: `${Math.max(16, ass.fontSize / 2)}px`,
  color: ass.primaryColor,
  WebkitTextStroke: `1px ${ass.outlineColor}`,
}));
const realtime = reactive({ language: "auto", text: "" });
const recording = ref(false);
let recorder: MediaRecorder | null = null;
let stream: MediaStream | null = null;
let uploadChain = Promise.resolve();
const clip = reactive({ input: "", output: "", start: 0, end: 10 });
const demux = reactive({ input: "", outputDir: "" });
const merge = reactive({ video: "", audio: "", output: "" });

async function loadAssSrt() {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const path = await open({ multiple: false, filters: [{ name: "SRT", extensions: ["srt"] }] });
  if (typeof path !== "string") return;
  ass.segments = parseSrt(await readTextFile(path));
}

async function exportAss() {
  const { save } = await import("@tauri-apps/plugin-dialog");
  const path = await save({ defaultPath: "subtitle.ass", filters: [{ name: "ASS", extensions: ["ass"] }] });
  if (!path) return;
  await writeTextFile(
    path,
    buildAss(ass.segments, {
      fontName: ass.fontName,
      fontSize: ass.fontSize,
      primaryColor: ass.primaryColor,
      outlineColor: ass.outlineColor,
      outline: 2,
      marginV: 40,
    }),
  );
  message.success("ASS 已导出");
}

async function startRealtime() {
  try {
    stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    const mimeType = MediaRecorder.isTypeSupported("audio/webm;codecs=opus")
      ? "audio/webm;codecs=opus"
      : "audio/webm";
    recorder = new MediaRecorder(stream, { mimeType });
    recorder.ondataavailable = (event) => {
      if (!event.data.size) return;
      uploadChain = uploadChain.then(async () => {
        const bytes = Array.from(new Uint8Array(await event.data.arrayBuffer()));
        const segments = await transcribeAudioChunk(bytes, "webm", realtime.language);
        const text = segments.map((segment) => segment.text).join(" ").trim();
        if (text) realtime.text += `${realtime.text ? "\n" : ""}${text}`;
      }).catch((error) => {
        message.error(`实时识别失败：${error}`);
      });
    };
    recorder.start(5000);
    recording.value = true;
  } catch (e) {
    message.error(`无法使用麦克风：${e}`);
  }
}

function stopRealtime() {
  recorder?.stop();
  stream?.getTracks().forEach((track) => track.stop());
  recorder = null;
  stream = null;
  recording.value = false;
}

async function runMatch() {
  busy.value = "match";
  try {
    await matchTextToSrt(match.srt, match.text, match.output);
    message.success("字幕匹配完成");
  } catch (e) {
    message.error(`匹配失败：${e}`);
  } finally {
    busy.value = null;
  }
}

async function runClip() {
  busy.value = "clip";
  try {
    await clipVideo(clip.input, clip.output, clip.start, clip.end);
    message.success("裁剪完成");
  } catch (e) {
    message.error(`裁剪失败：${e}`);
  } finally {
    busy.value = null;
  }
}

async function runDemux() {
  busy.value = "demux";
  try {
    const result = await separateMedia(demux.input, demux.outputDir);
    message.success(`已输出 ${result.video} 和 ${result.audio}`);
  } catch (e) {
    message.error(`分离失败：${e}`);
  } finally {
    busy.value = null;
  }
}

async function runMerge() {
  busy.value = "merge";
  try {
    await mergeVideoAudio(merge.video, merge.audio, merge.output);
    message.success("合并完成");
  } catch (e) {
    message.error(`合并失败：${e}`);
  } finally {
    busy.value = null;
  }
}
</script>

<style scoped>
.ass-grid {
  display: grid;
  grid-template-columns: 1fr 140px 160px 160px;
  gap: 8px;
}

.ass-preview {
  min-height: 100px;
  display: flex;
  align-items: center;
  justify-content: center;
  background: #20242b;
  border-radius: 8px;
  text-align: center;
}

.tool-card {
  display: flex;
  flex-direction: column;
  gap: 14px;
  margin-bottom: 16px;
}

.time-row {
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--vt-text-2);
  font-size: 13px;
}

.tool-card > .n-button {
  align-self: flex-start;
}
</style>
