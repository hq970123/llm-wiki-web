/**
 * Shared runtime for the web build's @tauri-apps/* shims.
 *
 * Only bundled in `vite build --mode web` (see vite.config.ts aliases);
 * the desktop build never sees this folder.
 */

import i18n from "@/i18n"

export const WEB_HEADER = { "X-LLM-Wiki": "1" } as const

function redirectToLogin(): void {
  if (typeof window !== "undefined" && !location.pathname.startsWith("/login")) {
    location.href = "/login"
  }
}

function networkErrorText(): string {
  return (i18n.language || "").toLowerCase().startsWith("zh")
    ? "无法连接服务器，请检查网络后重试"
    : "Cannot reach the server. Check your connection and try again."
}

/** fetch() against our own server: same-origin cookie + CSRF header. */
export async function apiFetch(path: string, init: RequestInit = {}): Promise<Response> {
  const headers = new Headers(init.headers)
  headers.set("X-LLM-Wiki", "1")
  let res: Response
  try {
    res = await nativeFetch(path, { ...init, headers, credentials: "same-origin" })
  } catch (err) {
    if ((err as { name?: string })?.name === "AbortError") throw err
    throw new Error(networkErrorText())
  }
  if (res.status === 401) {
    redirectToLogin()
    throw new Error("Not signed in")
  }
  return res
}

export async function apiJson<T = unknown>(path: string, body?: unknown, method = "POST"): Promise<T> {
  const res = await apiFetch(path, {
    method,
    headers: body === undefined ? undefined : { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  const text = await res.text()
  let data: unknown = null
  if (text) {
    try {
      data = JSON.parse(text)
    } catch {
      data = text
    }
  }
  if (!res.ok) {
    const err = data && typeof data === "object" && "error" in (data as Record<string, unknown>)
      ? (data as Record<string, unknown>).error
      : data ?? `HTTP ${res.status}`
    throw err
  }
  return data as T
}

/** The real browser fetch, captured before any patching. */
export const nativeFetch: typeof globalThis.fetch =
  typeof window !== "undefined" ? window.fetch.bind(window) : globalThis.fetch.bind(globalThis)

// ---------------------------------------------------------------- web config

export interface WebConfig {
  web: true
  version: string
  dataDir: string
  projectsDir: string
  uploadsDir: string
  exportsDir: string
  passwordFromEnv: boolean
  allowCli: boolean
  apiServerEnabled: boolean
}

let configPromise: Promise<WebConfig> | null = null
export function getWebConfig(): Promise<WebConfig> {
  if (!configPromise) {
    configPromise = apiJson<WebConfig>("/api/web/config", undefined, "GET").catch((e) => {
      configPromise = null
      throw e
    })
  }
  return configPromise
}

// ---------------------------------------------------------------- event bus (SSE)

type Handler = (event: { event: string; id: number; payload: unknown }) => void

const handlers = new Map<string, Set<Handler>>()
let source: EventSource | null = null
let nextId = 1

// Connection state, surfaced in the sidebar (window event "lw:connection").
let connected = true
let reportTimer: ReturnType<typeof setTimeout> | null = null
let retryTimer: ReturnType<typeof setTimeout> | null = null

function setConnected(next: boolean): void {
  if (reportTimer) {
    clearTimeout(reportTimer)
    reportTimer = null
  }
  const emit = () => {
    if (connected === next) return
    connected = next
    window.dispatchEvent(new CustomEvent("lw:connection", { detail: { connected: next } }))
  }
  // Brief blips (mobile network handover, server restart) shouldn't flash
  // a warning; report a drop only if it lasts a few seconds.
  if (next) emit()
  else reportTimer = setTimeout(emit, 3000)
}

export function isConnected(): boolean {
  return connected
}

function scheduleReconnect(ms: number): void {
  if (retryTimer) clearTimeout(retryTimer)
  retryTimer = setTimeout(() => {
    retryTimer = null
    ensureSource()
  }, ms)
}

function reconnectNow(): void {
  if (source && source.readyState !== EventSource.CLOSED) return
  source?.close()
  source = null
  if (retryTimer) clearTimeout(retryTimer)
  retryTimer = null
  ensureSource()
}

if (typeof window !== "undefined") {
  // Phones freeze background tabs and drop the stream; reconnect at once
  // when the page comes back instead of waiting for the retry timer.
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") reconnectNow()
  })
  window.addEventListener("online", reconnectNow)
  window.addEventListener("offline", () => setConnected(false))
}

// Last server event id seen ("<boot>:<seq>"), for gap replay on reconnect.
let lastEventId = ""

function isDuplicate(id: string): boolean {
  const [boot, seq] = id.split(":")
  const [lastBoot, lastSeq] = lastEventId.split(":")
  return boot === lastBoot && Number(seq) <= Number(lastSeq)
}

function ensureSource(): void {
  if (source || typeof EventSource === "undefined") return
  // Resume after the last event we saw so the server replays the gap
  // (EventSource's own retries send Last-Event-ID; a new one needs ?since=).
  const url = lastEventId ? `/api/events?since=${encodeURIComponent(lastEventId)}` : "/api/events"
  source = new EventSource(url, { withCredentials: true })
  source.onopen = () => setConnected(true)
  source.onmessage = (msg) => {
    if (msg.lastEventId) {
      if (isDuplicate(msg.lastEventId)) return
      lastEventId = msg.lastEventId
    }
    let parsed: { event?: string; payload?: unknown }
    try {
      parsed = JSON.parse(msg.data)
    } catch {
      return
    }
    const name = parsed.event
    if (!name) return
    const set = handlers.get(name)
    if (!set) return
    for (const h of Array.from(set)) {
      try {
        h({ event: name, id: nextId++, payload: parsed.payload })
      } catch (err) {
        console.error(`[web-shim] listener for ${name} failed`, err)
      }
    }
  }
  source.onerror = () => {
    setConnected(false)
    // EventSource reconnects by itself; if the session expired the
    // reconnect gets a 401 and the source closes — check and bounce.
    if (source && source.readyState === EventSource.CLOSED) {
      source = null
      void nativeFetch("/api/auth/status", { credentials: "same-origin" })
        .then((r) => r.json())
        .then((s) => {
          if (!s.authenticated) redirectToLogin()
          else scheduleReconnect(2000)
        })
        .catch(() => scheduleReconnect(5000))
    }
  }
}

export function subscribe(event: string, handler: Handler): () => void {
  ensureSource()
  let set = handlers.get(event)
  if (!set) {
    set = new Set()
    handlers.set(event, set)
  }
  set.add(handler)
  return () => {
    set!.delete(handler)
  }
}

/** Open the SSE connection early so no backend event is missed. */
export function connectEvents(): void {
  ensureSource()
}

// ---------------------------------------------------------------- downloads

/** Paths handed out by the save() shim; downloaded once a command wrote them. */
export const pendingDownloads = new Set<string>()

export function downloadServerFile(path: string, name?: string): void {
  const url = `/api/download?path=${encodeURIComponent(path)}${name ? `&name=${encodeURIComponent(name)}` : ""}`
  const a = document.createElement("a")
  a.href = url
  a.download = name ?? ""
  a.rel = "noopener"
  document.body.appendChild(a)
  a.click()
  a.remove()
}

export function assetUrl(path: string): string {
  return `/api/asset?path=${encodeURIComponent(path)}`
}
