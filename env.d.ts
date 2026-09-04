/// <reference types="vite/client" />

interface Window {
  __TAURI_INTERNALS__: {
    invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>
    listen: (event: string, cb: (data: unknown) => void) => Promise<() => void>
  }
}