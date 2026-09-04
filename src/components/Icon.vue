<template>
  <svg
    :width="size"
    :height="size"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    :stroke-width="stroke"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <template v-for="(shape, i) in shapes" :key="i">
      <rect v-if="shape[0] === 'rect'" v-bind="shape[1]" />
      <circle v-else-if="shape[0] === 'circle'" v-bind="shape[1]" />
      <path v-else :d="shape[1] as string" />
    </template>
  </svg>
</template>

<script setup lang="ts">
import { computed } from "vue";

type Attrs = Record<string, string | number>;
type Shape = ["path", string] | ["rect", Attrs] | ["circle", Attrs];

/**
 * 图标集（Feather/Lucide 风格，描边 24x24）
 * 按需引入，避免额外图标库依赖
 */
const ICONS: Record<string, Shape[]> = {
  film: [
    ["rect", { x: 2, y: 2, width: 20, height: 20, rx: 2.2 }],
    ["path", "M7 2v20M17 2v20M2 12h20M2 7h5M2 17h5M17 17h5M17 7h5"],
  ],
  list: [["path", "M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01"]],
  subtitle: [
    ["rect", { x: 2, y: 4, width: 20, height: 16, rx: 2 }],
    ["path", "M7 12h4M13 12h4M7 16h4M13 16h4"],
  ],
  settings: [
    ["circle", { cx: 12, cy: 12, r: 3 }],
    [
      "path",
      "M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z",
    ],
  ],
  folder: [
    ["path", "M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"],
  ],
  play: [["path", "M6 4l14 8-14 8V4z"]],
  x: [["path", "M18 6 6 18M6 6l12 12"]],
  check: [["path", "M20 6 9 17l-5-5"]],
  "chevron-down": [["path", "M6 9l6 6 6-6"]],
  "alert-circle": [
    ["circle", { cx: 12, cy: 12, r: 10 }],
    ["path", "M12 8v4M12 16h.01"],
  ],
  clock: [
    ["circle", { cx: 12, cy: 12, r: 10 }],
    ["path", "M12 6v6l4 2"],
  ],
  trash: [
    ["path", "M3 6h18M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2M10 11v6M14 11v6"],
  ],
  "rotate-cw": [["path", "M23 4v6h-6M20.49 15a9 9 0 1 1-2.12-9.36L23 10"]],
  "external-link": [
    ["path", "M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6M15 3h6v6M10 14 21 3"],
  ],
  download: [["path", "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M7 10l5 5 5-5M12 15V3"]],
  upload: [["path", "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M17 8l-5-5-5 5M12 3v12"]],
  mic: [
    ["path", "M12 1a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V4a3 3 0 0 0-3-3zM19 10v2a7 7 0 0 1-14 0v-2M12 19v4M8 23h8"],
  ],
  languages: [["path", "M5 8l6 6M4 14l6-6 2-3M2 5h12M7 2h1M22 22l-5-10-5 10M14 18h6"]],
  volume: [["path", "M11 5 6 9H2v6h4l5 4V5zM19.07 4.93a10 10 0 0 1 0 14.14M15.54 8.46a5 5 0 0 1 0 7.07"]],
  cpu: [
    ["rect", { x: 4, y: 4, width: 16, height: 16, rx: 2 }],
    ["rect", { x: 9, y: 9, width: 6, height: 6 }],
    ["path", "M9 1v3M15 1v3M9 20v3M15 20v3M20 9h3M20 14h3M1 9h3M1 14h3"],
  ],
  sliders: [["path", "M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6"]],
  save: [
    ["path", "M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2zM17 21v-8H7v8M7 3v5h8"],
  ],
  plus: [["path", "M12 5v14M5 12h14"]],
  refresh: [["path", "M1 4v6h6M3.51 15a9 9 0 1 0 2.13-9.36L1 10"]],
  zap: [["path", "M13 2 3 14h9l-1 8 10-12h-9l1-8z"]],
  file: [
    ["path", "M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8zM14 2v6h6"],
  ],
  globe: [
    ["circle", { cx: 12, cy: 12, r: 10 }],
    ["path", "M2 12h20M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"],
  ],
  terminal: [["path", "M4 17l6-6-6-6M12 19h8"]],
  key: [
    ["circle", { cx: 7.5, cy: 15.5, r: 5.5 }],
    ["path", "M21 2l-9.6 9.6M15.5 7.5l3 3L22 7l-3-3"],
  ],
  "arrow-left": [["path", "M19 12H5M12 19l-7-7 7-7"]],
};

const props = withDefaults(
  defineProps<{ name: string; size?: number | string; stroke?: number | string }>(),
  { size: 18, stroke: 2 },
);

const shapes = computed(() => ICONS[props.name] ?? []);
</script>
