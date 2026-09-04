<template>
  <n-config-provider :theme-overrides="themeOverrides">
    <n-message-provider placement="bottom-right">
      <div class="app-shell">
        <AppSidebar />
        <main class="app-main">
          <div class="app-content">
            <HomePage v-if="store.currentPage === 'home'" />
            <TasksPage v-else-if="store.currentPage === 'tasks'" />
            <SubtitlePage v-else-if="store.currentPage === 'subtitle'" />
            <ToolsPage v-else-if="store.currentPage === 'tools'" />
            <SettingsPage v-else-if="store.currentPage === 'settings'" />
          </div>
        </main>
      </div>
    </n-message-provider>
  </n-config-provider>
</template>

<script setup lang="ts">
import { onMounted } from "vue";
import { NConfigProvider, NMessageProvider } from "naive-ui";
import type { GlobalThemeOverrides } from "naive-ui";
import AppSidebar from "./components/AppSidebar.vue";
import HomePage from "./views/HomePage.vue";
import TasksPage from "./views/TasksPage.vue";
import SubtitlePage from "./views/SubtitlePage.vue";
import ToolsPage from "./views/ToolsPage.vue";
import SettingsPage from "./views/SettingsPage.vue";
import { store, restorePersistentTasks } from "./lib/store";
import {
  checkFfmpeg,
  loadConfig,
  loadDialectSpecs,
  listPersistentTasks,
  getRuntimeInfo,
} from "./lib/api";

/** Naive UI 主题定制：对齐设计令牌 */
const themeOverrides: GlobalThemeOverrides = {
  common: {
    primaryColor: "#4f5bd5",
    primaryColorHover: "#6371e8",
    primaryColorPressed: "#4049b8",
    primaryColorSuppl: "#4f5bd5",
    successColor: "#1a9e60",
    errorColor: "#d4305a",
    warningColor: "#d97706",
    borderRadius: "8px",
    borderRadiusSmall: "6px",
    fontFamily:
      '-apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei", sans-serif',
  },
  Button: {
    fontWeight: "500",
  },
  Card: {
    borderRadius: "10px",
  },
};

onMounted(async () => {
  // 启动时检测环境 + 载入配置
  try {
    const version = await checkFfmpeg();
    store.ffmpeg = { status: "ok", version };
  } catch {
    store.ffmpeg = { status: "missing", version: "" };
  }
  try {
    store.config = await loadConfig();
  } catch {
    store.config = null;
  }
  try {
    store.runtime = await getRuntimeInfo();
  } catch {
    store.runtime = null;
  }
  try {
    store.dialectSpecs = await loadDialectSpecs();
  } catch {
    store.dialectSpecs = [];
  }
  try {
    restorePersistentTasks(await listPersistentTasks());
  } catch {
    // 历史任务读取失败不阻塞工作台；详细错误已由后端日志记录。
  }
});
</script>

<style scoped>
.app-shell {
  display: flex;
  height: 100vh;
  overflow: hidden;
}

.app-main {
  flex: 1;
  min-width: 0;
  overflow-y: auto;
}

.app-content {
  max-width: 1024px;
  margin: 0 auto;
  padding: 32px;
}
</style>
