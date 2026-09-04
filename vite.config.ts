import { configDefaults, defineConfig } from 'vitest/config'
import vue from '@vitejs/plugin-vue'

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [vue()],
  test: {
    // .refactor-backup/ 是重构回滚备份（冻结快照），里面的测试副本不参与门禁
    exclude: [...configDefaults.exclude, '**/.refactor-backup/**']
  },
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // 不监听 Rust 构建目录：target/ 下成千上万个构建产物频繁变化，
      // 既浪费 CPU，又会在 cargo 写入 DLL 时因文件锁导致 EBUSY 崩溃。
      // 也不要开启 usePolling（100ms 轮询全项目，CPU 常驻高位）。
      ignored: ['**/src-tauri/**']
    }
  }
})