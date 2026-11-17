/**
 * Web shim for `@tauri-apps/api/event`: backend `app.emit(...)` events
 * arrive over one shared EventSource on /api/events.
 */
import { apiJson, subscribe } from "./runtime"

export type EventName = string
export type UnlistenFn = () => void
export interface Event<T> {
  event: EventName
  id: number
  payload: T
}
export type EventCallback<T> = (event: Event<T>) => void
export interface Options {
  target?: unknown
}

export const TauriEvent = {
  WINDOW_RESIZED: "tauri://resize",
  WINDOW_MOVED: "tauri://move",
  WINDOW_CLOSE_REQUESTED: "tauri://close-requested",
  WINDOW_DESTROYED: "tauri://destroyed",
  WINDOW_FOCUS: "tauri://focus",
  WINDOW_BLUR: "tauri://blur",
  WINDOW_SCALE_FACTOR_CHANGED: "tauri://scale-change",
  WINDOW_THEME_CHANGED: "tauri://theme-changed",
  DRAG_ENTER: "tauri://drag-enter",
  DRAG_OVER: "tauri://drag-over",
  DRAG_DROP: "tauri://drag-drop",
  DRAG_LEAVE: "tauri://drag-leave",
} as const

export async function listen<T>(event: EventName, handler: EventCallback<T>, _options?: Options): Promise<UnlistenFn> {
  return subscribe(event, handler as (e: Event<unknown>) => void)
}

export async function once<T>(event: EventName, handler: EventCallback<T>, _options?: Options): Promise<UnlistenFn> {
  const off = subscribe(event, (e) => {
    off()
    ;(handler as (e: Event<unknown>) => void)(e)
  })
  return off
}

export async function emit<T>(event: string, payload?: T): Promise<void> {
  await apiJson("/api/emit", { event, payload: payload ?? null })
}

export async function emitTo<T>(_target: unknown, event: string, payload?: T): Promise<void> {
  await emit(event, payload)
}
