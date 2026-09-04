<template>
  <div
    class="drop-zone"
    :class="{ 'drop-active': isDragging }"
    @click="triggerFilePicker"
  >
    <div class="drop-content">
      <div class="drop-icon-wrap">
        <Icon name="upload" :size="22" :stroke="1.8" />
      </div>
      <p class="drop-text">点击选择视频，或将文件拖入窗口</p>
      <p class="drop-hint">支持批量添加 · MP4 / MKV / AVI / MOV / WMV / FLV</p>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, onUnmounted } from "vue";
import { listen } from "@tauri-apps/api/event";
import { pickVideoFiles } from "../lib/api";
import Icon from "./Icon.vue";

const emit = defineEmits<{
  filesSelected: [paths: string[]]
}>();

const isDragging = ref(false);

const triggerFilePicker = async () => {
  try {
    const paths = await pickVideoFiles();
    if (paths.length > 0) {
      emit("filesSelected", paths);
    }
  } catch (error) {
    console.error("选择文件失败:", error);
  }
};

let unlisten: (() => void) | undefined;

onMounted(async () => {
  try {
    unlisten = await listen<{ type: string; paths: string[] }>(
      "tauri-drag-drop",
      (event) => {
        const { type, paths } = event.payload;
        if (type === "enter") {
          isDragging.value = true;
        } else if (type === "leave") {
          isDragging.value = false;
        } else if (type === "drop") {
          isDragging.value = false;
          if (paths?.length) {
            emit("filesSelected", paths);
          }
        }
      },
    );
  } catch (e) {
    console.warn("拖拽事件监听失败:", e);
  }
});

onUnmounted(() => {
  unlisten?.();
});
</script>

<style scoped>
.drop-zone {
  border: 1.5px dashed var(--vt-border-strong);
  border-radius: var(--vt-radius);
  padding: 40px 24px;
  text-align: center;
  cursor: pointer;
  background: var(--vt-surface-sunken);
  transition: border-color 0.15s ease, background 0.15s ease;
}

.drop-zone:hover {
  border-color: var(--vt-accent);
  background: var(--vt-accent-weak);
}

.drop-active {
  border-color: var(--vt-accent);
  background: var(--vt-accent-weak);
  box-shadow: 0 0 0 3px rgba(79, 91, 213, 0.12);
}

.drop-content {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
}

.drop-icon-wrap {
  width: 44px;
  height: 44px;
  border-radius: 50%;
  background: var(--vt-accent-weak);
  color: var(--vt-accent);
  display: flex;
  align-items: center;
  justify-content: center;
  margin-bottom: 4px;
}

.drop-text {
  font-size: 14.5px;
  font-weight: 500;
  color: var(--vt-text);
  margin: 0;
}

.drop-hint {
  font-size: 12.5px;
  color: var(--vt-text-3);
  margin: 0;
}
</style>
