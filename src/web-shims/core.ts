/**
 * Web shim for `@tauri-apps/api/core`.
 * invoke(cmd, args) -> POST /api/invoke/<cmd> (same Rust command, JSON args).
 */
import { apiJson, assetUrl, connectEvents, downloadServerFile, pendingDownloads } from "./runtime"
import "./web-ui"

connectEvents()

export type InvokeArgs = Record<string, unknown> | number[] | ArrayBuffer | Uint8Array

function collectStrings(value: unknown, out: string[]): void {
  if (typeof value === "string") out.push(value)
  else if (Array.isArray(value)) value.forEach((v) => collectStrings(v, out))
  else if (value && typeof value === "object") Object.values(value).forEach((v) => collectStrings(v, out))
}

export async function invoke<T>(cmd: string, args: InvokeArgs = {}, _options?: unknown): Promise<T> {
  const result = await apiJson<T>(`/api/invoke/${encodeURIComponent(cmd)}`, args ?? {})
  // A path returned by the save() shim was just written by a command
  // (e.g. export_project_archive): hand the file to the browser.
  if (pendingDownloads.size > 0) {
    const strings: string[] = []
    collectStrings(args, strings)
    for (const s of strings) {
      if (pendingDownloads.has(s)) {
        pendingDownloads.delete(s)
        downloadServerFile(s, s.split("/").pop())
      }
    }
  }
  return result
}

export function convertFileSrc(filePath: string, _protocol = "asset"): string {
  return assetUrl(filePath)
}

export function isTauri(): boolean {
  return false
}

export async function transformCallback(): Promise<number> {
  return 0
}

export class Channel<T = unknown> {
  id = 0
  onmessage: (response: T) => void = () => {}
}

export class Resource {
  constructor(public rid: number) {}
  async close(): Promise<void> {}
}

export class PluginListener {
  async unregister(): Promise<void> {}
}
