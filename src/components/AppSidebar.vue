<template>
  <aside class="sidebar">
    <!-- 品牌区 -->
    <div class="brand">
      <div class="brand-logo">
        <Icon name="film" :size="20" :stroke="1.8" />
      </div>
      <div class="brand-text">
        <div class="brand-name">VideoTrans</div>
        <div class="brand-sub">视频翻译工作台</div>
      </div>
    </div>

    <!-- 导航 -->
    <nav class="nav">
      <div
        v-for="item in navItems"
        :key="item.key"
        class="nav-item"
        :class="{ active: store.currentPage === item.key }"
        @click="navigateTo(item.key as PageKey)"
      >
        <Icon :name="item.icon" :size="17" />
        <span>{{ item.label }}</span>
        <span v-if="item.key === 'tasks' && taskStats.running + taskStats.pending > 0" class="nav-badge">
          {{ taskStats.running + taskStats.pending }}
        </span>
      </div>
    </nav>

    <!-- 底部环境状态 -->
    <div class="sidebar-footer">
      <div class="env-row">
        <span class="vt-dot" :class="envDotClass"></span>
        <span class="env-text">{{ envText }}</span>
      </div>
      <div class="version">v0.1.0</div>
    </div>
  </aside>
</template>

<script setup lang="ts">
import { computed } from "vue";
import { store, taskStats, navigateTo } from "../lib/store";
import type { PageKey } from "../lib/store";
import Icon from "./Icon.vue";

const navItems = [
  { key: "home", label: "工作台", icon: "zap" },
  { key: "tasks", label: "任务队列", icon: "list" },
  { key: "subtitle", label: "字幕编辑", icon: "subtitle" },
  { key: "tools", label: "媒体工具", icon: "sliders" },
  { key: "settings", label: "设置", icon: "settings" },
];

const envDotClass = computed(() => {
  if (store.ffmpeg.status === "ok") return "ok";
  if (store.ffmpeg.status === "missing") return "err";
  return "warn";
});

const envText = computed(() => {
  if (store.ffmpeg.status === "ok") return "FFmpeg 就绪";
  if (store.ffmpeg.status === "missing") return "FFmpeg 未找到";
  return "检测环境中…";
});
</script>

<style scoped>
.sidebar {
  width: var(--vt-sidebar-w);
  flex-shrink: 0;
  height: 100vh;
  background: var(--vt-surface);
  border-right: 1px solid var(--vt-border);
  display: flex;
  flex-direction: column;
  padding: 20px 12px 16px;
}

.brand {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 0 10px 20px;
  border-bottom: 1px solid var(--vt-border);
}

.brand-logo {
  width: 36px;
  height: 36px;
  border-radius: 9px;
  background: linear-gradient(135deg, var(--vt-accent), #7c8cf8);
  color: #fff;
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}

.brand-name {
  font-size: 15px;
  font-weight: 700;
  letter-spacing: 0.2px;
  line-height: 1.3;
}

.brand-sub {
  font-size: 11.5px;
  color: var(--vt-text-3);
  line-height: 1.3;
}

.nav {
  flex: 1;
  padding-top: 16px;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.nav-item {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 9px 12px;
  border-radius: var(--vt-radius-sm);
  font-size: 13.5px;
  font-weight: 500;
  color: var(--vt-text-2);
  cursor: pointer;
  transition: background 0.15s ease, color 0.15s ease;
}

.nav-item:hover {
  background: var(--vt-bg);
  color: var(--vt-text);
}

.nav-item.active {
  background: var(--vt-accent-weak);
  color: var(--vt-accent);
}

.nav-badge {
  margin-left: auto;
  min-width: 18px;
  height: 18px;
  padding: 0 5px;
  border-radius: 9px;
  background: var(--vt-accent);
  color: #fff;
  font-size: 11px;
  font-weight: 600;
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.nav-item.active .nav-badge {
  background: var(--vt-accent);
}

.sidebar-footer {
  border-top: 1px solid var(--vt-border);
  padding: 12px 10px 0;
}

.env-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.env-text {
  font-size: 12.5px;
  color: var(--vt-text-2);
}

.version {
  margin-top: 6px;
  font-size: 11.5px;
  color: var(--vt-text-3);
}
</style>
