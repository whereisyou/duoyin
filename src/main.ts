import { createApp } from 'vue'
import App from './App.vue'
import './style.css'
import { logFrontend } from './lib/api'

// 前端异常直达后端日志文件：「莫名报错」必须留痕，不靠用户转述
window.addEventListener('error', (e) => {
  logFrontend('error', `${e.message} @ ${e.filename}:${e.lineno}`)
})
window.addEventListener('unhandledrejection', (e) => {
  logFrontend('error', `unhandled rejection: ${e.reason}`)
})

createApp(App).mount('#app')
