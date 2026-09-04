<template>
  <div class="api-test">
    <n-button size="small" :loading="testing" @click="runTest">测试连通</n-button>
    <span v-if="result" class="test-result" :class="result.ok ? 'ok' : 'fail'">
      {{ result.ok ? `✓ ${result.msg}` : `✗ ${result.msg}` }}
    </span>
  </div>
</template>

<script setup lang="ts">
import { ref } from "vue";
import { NButton } from "naive-ui";
import { testApiEndpoint, testApiReachable } from "../lib/api";
import type { AppConfig } from "../lib/types";
import type { FieldDef } from "../lib/engines";

/**
 * 通用 API 连通性测试按钮（统一组件，STT/翻译/TTS 复用）：
 * - 地址：field.testUrl（固定端点，如 OpenAI）优先，否则取 field.key 对应的配置值
 * - Key：同组 fields 中 type=password 的字段
 * - 模型：同组 fields 中 key 以 _model 结尾的字段（chat 模式用）
 * - 模式：field.testMode
 *     chat（默认）    → POST chat/completions，验证鉴权+模型
 *     reachable       → GET，只验网络通路（Whisper/CosyVoice 等非 chat 接口）
 */
const props = defineProps<{ cfg: AppConfig; fields: FieldDef[]; field: FieldDef }>();

const testing = ref(false);
const result = ref<{ ok: boolean; msg: string } | null>(null);

async function runTest() {
  const url = (props.field.testUrl ?? String(props.cfg[props.field.key] ?? "")).trim();
  if (!url) {
    result.value = { ok: false, msg: "请先填写 API 地址" };
    return;
  }
  try {
    const u = new URL(url);
    if (u.protocol !== "http:" && u.protocol !== "https:") throw new Error();
  } catch {
    result.value = { ok: false, msg: "URL 格式不合法" };
    return;
  }

  const keyField = props.fields.find((f) => f.type === "password");
  const modelField = props.fields.find((f) => f.key.endsWith("_model"));
  const apiKey = keyField ? String(props.cfg[keyField.key] ?? "").trim() : "";
  const model = modelField ? String(props.cfg[modelField.key] ?? "").trim() : "";

  testing.value = true;
  result.value = null;
  try {
    const msg =
      props.field.testMode === "reachable"
        ? await testApiReachable(url, apiKey)
        : `连通正常（${await testApiEndpoint(url, apiKey, model)}）`;
    result.value = { ok: true, msg };
  } catch (e) {
    result.value = { ok: false, msg: String(e) };
  } finally {
    testing.value = false;
  }
}
</script>

<style scoped>
.api-test {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 8px;
}

.test-result {
  font-size: 12px;
  word-break: break-all;
}

.test-result.ok {
  color: var(--vt-success, #18a058);
}

.test-result.fail {
  color: var(--vt-error, #d03050);
}
</style>
