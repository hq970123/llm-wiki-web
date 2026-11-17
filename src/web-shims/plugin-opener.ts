/** Web shim for `@tauri-apps/plugin-opener`. */
import { assetUrl, getWebConfig } from "./runtime"

export async function openUrl(url: string | URL, _openWith?: string): Promise<void> {
  window.open(String(url), "_blank", "noopener,noreferrer")
}

/** Opens a server file in a new browser tab (inline for PDFs/images/text). */
export async function openPath(path: string, _openWith?: string): Promise<void> {
  if (/^https?:\/\//i.test(path)) return openUrl(path)
  const cfg = await getWebConfig().catch(() => null)
  if (cfg && !path.startsWith(cfg.dataDir)) {
    throw new Error("Only files inside the data directory can be opened")
  }
  window.open(assetUrl(path), "_blank", "noopener")
}

export async function revealItemInDir(_path: string | string[]): Promise<void> {
  throw new Error("Revealing files in a file manager is not available in the web version")
}
