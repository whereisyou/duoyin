<template>
  <div>
    <label class="vt-field-label">{{ label }}</label>
    <div class="picker-row">
      <n-input :value="modelValue" readonly :placeholder="placeholder" />
      <n-button @click="choose">选择</n-button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { NButton, NInput } from "naive-ui";

const props = withDefaults(
  defineProps<{
    modelValue: string;
    label: string;
    mode: "file" | "dir" | "save";
    extension?: string;
    placeholder?: string;
  }>(),
  { extension: "", placeholder: "请选择路径" },
);
const emit = defineEmits<{ "update:modelValue": [value: string] }>();

async function choose() {
  const dialog = await import("@tauri-apps/plugin-dialog");
  if (props.mode === "save") {
    const value = await dialog.save({
      filters: props.extension
        ? [{ name: props.extension.toUpperCase(), extensions: [props.extension] }]
        : undefined,
    });
    if (value) emit("update:modelValue", value);
    return;
  }
  const value = await dialog.open({ directory: props.mode === "dir", multiple: false });
  if (typeof value === "string") emit("update:modelValue", value);
}
</script>

<style scoped>
.picker-row {
  display: flex;
  gap: 8px;
}

.picker-row :deep(.n-input) {
  flex: 1;
}
</style>
