<template>
  <div class="config-fields">
    <div v-for="f in fields" :key="f.key" class="field">
      <!-- 开关型 -->
      <template v-if="f.type === 'switch'">
        <label class="switch-row">
          <n-switch
            :value="!!cfg[f.key]"
            size="small"
            @update:value="(v: boolean) => set(f, v)"
          />
          <span class="switch-label">{{ f.label }}</span>
        </label>
        <p v-if="f.hint" class="field-hint">{{ f.hint }}</p>
      </template>

      <!-- 文本 / 密码型 -->
      <template v-else>
        <label class="vt-field-label">{{ f.label }}</label>
        <div class="row">
          <n-input
            :value="String(cfg[f.key] ?? '')"
            :type="f.type === 'password' ? 'password' : 'text'"
            :placeholder="f.placeholder"
            :show-password-on="f.type === 'password' ? 'click' : undefined"
            clearable
            @update:value="(v: string) => set(f, v)"
          />
          <n-button v-if="f.browse" @click="pick(f)">浏览</n-button>
        </div>
        <ApiTestButton v-if="f.testable" :cfg="cfg" :fields="fields" :field="f" />
        <p v-if="f.hint" class="field-hint">{{ f.hint }}</p>
      </template>
    </div>
  </div>
</template>

<script setup lang="ts">
import { NButton, NInput, NSwitch, useMessage } from "naive-ui";
import ApiTestButton from "./ApiTestButton.vue";
import type { AppConfig } from "../lib/types";
import type { FieldDef } from "../lib/engines";

const props = defineProps<{ fields: FieldDef[]; cfg: AppConfig }>();
const message = useMessage();

function set(f: FieldDef, v: string | boolean) {
  if (f.type === "number") {
    const n = Math.max(0, Math.floor(Number(v)));
    if (!Number.isFinite(n)) return;
    (props.cfg as unknown as Record<string, string | boolean | number>)[f.key] = n;
    return;
  }
  // URL / Key 类字段输入时自动去掉首尾空格，避免复制粘贴带入空白导致请求失败
  if (typeof v === "string" && (f.testable || f.type === "password")) {
    v = v.trim();
  }
  (props.cfg as unknown as Record<string, string | boolean | number>)[f.key] = v;
}

async function pick(f: FieldDef) {
  try {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open(
      f.browse === "dir"
        ? { directory: true, multiple: false, title: `选择${f.label}` }
        : {
            directory: false,
            multiple: false,
            title: `选择${f.label}`,
            filters: f.extensions?.length ? [{ name: f.label, extensions: f.extensions }] : undefined,
          },
    );
    if (picked) set(f, picked as string);
  } catch (e) {
    message.error(`选择失败：${e}`);
  }
}
</script>

<style scoped>
.config-fields > .field + .field {
  margin-top: 16px;
}

.row {
  display: flex;
  gap: 8px;
}

.row :deep(.n-input) {
  flex: 1;
}

.field-hint {
  margin: 6px 0 0;
  font-size: 12px;
  color: var(--vt-text-3);
}

.switch-row {
  display: inline-flex;
  align-items: center;
  gap: 10px;
  cursor: pointer;
}

.switch-label {
  font-size: 13.5px;
  color: var(--vt-text);
}
</style>
