/** Web shim for `@tauri-apps/api/window`: there is no native window. */
import type { UnlistenFn } from "./event"

export type Theme = "light" | "dark"

class WebWindow {
  label = "main"
  async setTheme(_theme?: Theme | null): Promise<void> {}
  async setBackgroundColor(_color: unknown): Promise<void> {}
  async theme(): Promise<Theme | null> {
    return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light"
  }
  async onThemeChanged(_cb: unknown): Promise<UnlistenFn> {
    return () => {}
  }
  async onCloseRequested(_cb: unknown): Promise<UnlistenFn> {
    return () => {}
  }
  async onFocusChanged(_cb: unknown): Promise<UnlistenFn> {
    return () => {}
  }
  async listen(_event: string, _cb: unknown): Promise<UnlistenFn> {
    return () => {}
  }
  async setTitle(title: string): Promise<void> {
    document.title = title
  }
  async show(): Promise<void> {}
  async hide(): Promise<void> {}
  async close(): Promise<void> {}
  async minimize(): Promise<void> {}
  async maximize(): Promise<void> {}
  async unmaximize(): Promise<void> {}
  async toggleMaximize(): Promise<void> {}
  async isMaximized(): Promise<boolean> {
    return false
  }
  async isFullscreen(): Promise<boolean> {
    return !!document.fullscreenElement
  }
  async setFullscreen(on: boolean): Promise<void> {
    if (on) await document.documentElement.requestFullscreen?.()
    else if (document.fullscreenElement) await document.exitFullscreen?.()
  }
  async setFocus(): Promise<void> {
    window.focus()
  }
  async startDragging(): Promise<void> {}
}

const current = new WebWindow()
export function getCurrentWindow(): WebWindow {
  return current
}
export function getAllWindows(): WebWindow[] {
  return [current]
}
export const Window = WebWindow
