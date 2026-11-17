/**
 * Web shim for `@tauri-apps/plugin-dialog`.
 *
 * - open({ directory: false }) -> browser file picker; the files are
 *   uploaded to DATA_DIR/uploads/<stamp>/ and their *server* paths are
 *   returned, so the existing copy_file / import flow is unchanged.
 * - open({ directory: true })  -> server-side folder browser (inside
 *   DATA_DIR) that can also upload a local folder.
 * - save()                     -> a path in DATA_DIR/exports; once a
 *   command has written it, the browser downloads it (see core.ts).
 * - message/ask/confirm        -> window.alert / window.confirm.
 */
import { getWebConfig, pendingDownloads } from "./runtime"
import { pickFiles, pickServerFolder, uploadFiles } from "./web-ui"

export interface DialogFilter {
  name: string
  extensions: string[]
}

export interface OpenDialogOptions {
  title?: string
  filters?: DialogFilter[]
  defaultPath?: string
  multiple?: boolean
  directory?: boolean
  recursive?: boolean
  canCreateDirectories?: boolean
  createDirectories?: boolean
}

export interface SaveDialogOptions {
  title?: string
  filters?: DialogFilter[]
  defaultPath?: string
  canCreateDirectories?: boolean
}

export interface MessageDialogOptions {
  title?: string
  kind?: "info" | "warning" | "error"
  okLabel?: string
  cancelLabel?: string
}

export async function open(options: OpenDialogOptions = {}): Promise<string | string[] | null> {
  if (options.directory) {
    const picked = await pickServerFolder({ title: options.title, multiple: options.multiple })
    if (!picked) return null
    return options.multiple ? [picked] : picked
  }
  const accept = (options.filters ?? [])
    .flatMap((f) => f.extensions)
    .filter((e) => e && e !== "*")
    .map((e) => `.${e}`)
    .join(",")
  const files = await pickFiles({ multiple: !!options.multiple, accept, directory: false })
  if (!files || files.length === 0) return null
  const uploaded = await uploadFiles(files, options.title)
  if (!uploaded) return null
  return options.multiple ? uploaded.paths : uploaded.paths[0] ?? null
}

function sanitizeName(name: string): string {
  const base = name.split(/[\\/]/).pop() || "download"
  return base.replace(/[^\p{L}\p{N}._ -]+/gu, "_").slice(0, 180) || "download"
}

export async function save(options: SaveDialogOptions = {}): Promise<string | null> {
  const cfg = await getWebConfig()
  let name = sanitizeName(options.defaultPath ?? "")
  const ext = options.filters?.[0]?.extensions?.[0]
  if (name === "download" && ext) name = `export.${ext}`
  const stamp = new Date().toISOString().replace(/[-:]/g, "").replace(/\..*/, "").replace("T", "-")
  const path = `${cfg.exportsDir}/${stamp}-${name}`
  pendingDownloads.add(path)
  return path
}

export async function message(msg: string, options?: string | MessageDialogOptions): Promise<void> {
  const title = typeof options === "string" ? options : options?.title
  window.alert(title ? `${title}\n\n${msg}` : msg)
}

export async function ask(msg: string, options?: string | MessageDialogOptions): Promise<boolean> {
  const title = typeof options === "string" ? options : options?.title
  return window.confirm(title ? `${title}\n\n${msg}` : msg)
}

export async function confirm(msg: string, options?: string | MessageDialogOptions): Promise<boolean> {
  return ask(msg, options)
}
