/**
 * Browser-only UI pieces for the web build: file picking + upload, the
 * server-side folder browser, a sign-out button, and rewriting of the
 * desktop app's localhost service URLs to the authenticated server.
 */
import { apiFetch, apiJson, getWebConfig, nativeFetch } from "./runtime"

import i18n from "@/i18n"

const T_ZH = {
      chooseFolder: "选择服务器上的文件夹",
      select: "选择此文件夹",
      cancel: "取消",
      up: "上一级",
      newFolder: "新建文件夹",
      newFolderPrompt: "新文件夹名称：",
      uploadFolder: "从本机上传文件夹…",
      uploading: "正在上传",
      empty: "（没有子文件夹）",
      project: "项目",
      signOut: "退出登录",
      changePw: "修改密码",
      curPw: "当前密码",
      newPw: "新密码（至少 8 位）",
      confirmPw: "确认新密码",
      save: "保存",
      pwMismatch: "两次输入的新密码不一致",
      pwShort: "新密码至少 8 位",
      pwDone: "密码已修改",
      hint: "所有数据保存在服务器的数据目录中。",
      uploadFailed: "上传失败",
      uploadNetwork: "上传失败：网络中断",
    }
const T_EN: typeof T_ZH = {
      chooseFolder: "Choose a folder on the server",
      select: "Select this folder",
      cancel: "Cancel",
      up: "Up",
      newFolder: "New folder",
      newFolderPrompt: "New folder name:",
      uploadFolder: "Upload a folder from this computer…",
      uploading: "Uploading",
      empty: "(no sub-folders)",
      project: "project",
      signOut: "Sign out",
      changePw: "Change password",
      curPw: "Current password",
      newPw: "New password (8+ characters)",
      confirmPw: "Confirm new password",
      save: "Save",
      pwMismatch: "New passwords do not match",
      pwShort: "New password must be at least 8 characters",
      pwDone: "Password changed",
      hint: "Everything lives in the server's data directory.",
      uploadFailed: "Upload failed",
      uploadNetwork: "Upload failed (network error)",
    }

/** Shell strings follow the app's current UI language (Settings → Interface). */
const T: typeof T_ZH = new Proxy({} as typeof T_ZH, {
  get: (_t, key: string) =>
    ((i18n.language || "").toLowerCase().startsWith("zh") ? T_ZH : T_EN)[key as keyof typeof T_ZH],
})

// ---------------------------------------------------------------- styles

let styled = false
function ensureStyles(): void {
  if (styled || typeof document === "undefined") return
  styled = true
  const css = `
  .lw-overlay{position:fixed;inset:0;background:rgba(0,0,0,.45);z-index:2147483000;display:flex;align-items:center;justify-content:center;font:14px/1.45 system-ui,-apple-system,"Segoe UI",Roboto,"PingFang SC","Microsoft YaHei",sans-serif}
  .lw-modal{width:min(560px,94vw);max-height:80vh;display:flex;flex-direction:column;background:var(--background,#fff);color:var(--foreground,#111);border:1px solid var(--border,#e5e7eb);border-radius:12px;box-shadow:0 20px 50px rgba(0,0,0,.25);overflow:hidden}
  .lw-head{padding:14px 16px 6px;font-weight:600;font-size:15px}
  .lw-path{padding:0 16px 8px;font:12px ui-monospace,SFMono-Regular,Menlo,monospace;opacity:.7;word-break:break-all}
  .lw-list{flex:1;overflow:auto;border-top:1px solid var(--border,#e5e7eb);border-bottom:1px solid var(--border,#e5e7eb);min-height:180px}
  .lw-item{display:flex;gap:8px;align-items:center;padding:7px 16px;cursor:pointer;user-select:none}
  .lw-item:hover{background:rgba(127,127,127,.12)}
  .lw-badge{font-size:11px;padding:1px 6px;border-radius:999px;background:rgba(37,99,235,.15);color:#2563eb}
  .lw-empty{padding:16px;opacity:.6}
  .lw-foot{display:flex;flex-wrap:wrap;gap:8px;padding:10px 16px;align-items:center}
  .lw-foot .lw-spacer{flex:1}
  .lw-btn{border:1px solid var(--border,#d1d5db);background:transparent;color:inherit;border-radius:8px;padding:6px 10px;cursor:pointer;font:inherit}
  .lw-btn:hover{background:rgba(127,127,127,.12)}
  .lw-primary{background:#2563eb;border-color:#2563eb;color:#fff}
  .lw-primary:hover{background:#1d4ed8}
  .lw-hint{padding:0 16px 10px;font-size:12px;opacity:.6}
  .lw-progress{position:fixed;left:50%;bottom:24px;transform:translateX(-50%);z-index:2147483001;background:#111827;color:#fff;padding:8px 14px;border-radius:999px;font:13px system-ui,sans-serif;box-shadow:0 6px 20px rgba(0,0,0,.3)}
  .lw-form{padding:6px 16px 4px;display:flex;flex-direction:column;gap:8px}
  .lw-form input{border:1px solid var(--border,#d1d5db);border-radius:8px;padding:7px 10px;font:inherit;background:transparent;color:inherit}
  .lw-err{color:#dc2626;font-size:12px;min-height:16px}
  `
  const el = document.createElement("style")
  el.textContent = css
  document.head.appendChild(el)
}

// ---------------------------------------------------------------- file picking

export function pickFiles(opts: { multiple: boolean; accept?: string; directory: boolean }): Promise<File[] | null> {
  return new Promise((resolve) => {
    const input = document.createElement("input")
    input.type = "file"
    input.multiple = opts.multiple || opts.directory
    if (opts.accept) input.accept = opts.accept
    if (opts.directory) {
      input.setAttribute("webkitdirectory", "")
      input.setAttribute("directory", "")
    }
    input.style.display = "none"
    let settled = false
    const done = (files: File[] | null) => {
      if (settled) return
      settled = true
      input.remove()
      resolve(files)
    }
    input.addEventListener("change", () => done(input.files ? Array.from(input.files) : null))
    input.addEventListener("cancel", () => done(null))
    document.body.appendChild(input)
    input.click()
  })
}

export interface UploadResult {
  root: string
  paths: string[]
  dirs: string[]
}

export async function uploadFiles(files: File[], _label?: string, dir?: string): Promise<UploadResult | null> {
  ensureStyles()
  const bar = document.createElement("div")
  bar.className = "lw-progress"
  bar.textContent = `${T.uploading}… 0%`
  document.body.appendChild(bar)
  try {
    const form = new FormData()
    for (const f of files) {
      const rel = (f as File & { webkitRelativePath?: string }).webkitRelativePath || f.name
      form.append("relpath", rel)
      form.append("file", f, f.name)
    }
    const url = `/api/upload${dir ? `?dir=${encodeURIComponent(dir)}` : ""}`
    // XHR for upload progress.
    const result = await new Promise<UploadResult>((resolve, reject) => {
      const xhr = new XMLHttpRequest()
      xhr.open("POST", url)
      xhr.withCredentials = true
      xhr.setRequestHeader("X-LLM-Wiki", "1")
      xhr.upload.onprogress = (e) => {
        if (e.lengthComputable) bar.textContent = `${T.uploading}… ${Math.round((e.loaded / e.total) * 100)}%`
      }
      xhr.onload = () => {
        if (xhr.status === 401) {
          location.href = "/login"
          return reject(new Error("Not signed in"))
        }
        let data: unknown
        try {
          data = JSON.parse(xhr.responseText)
        } catch {
          data = { error: xhr.responseText }
        }
        if (xhr.status >= 200 && xhr.status < 300) resolve(data as UploadResult)
        else reject(new Error((data as { error?: string }).error ?? `${T.uploadFailed} (${xhr.status})`))
      }
      xhr.onerror = () => reject(new Error(T.uploadNetwork))
      xhr.send(form)
    })
    return result
  } finally {
    bar.remove()
  }
}

// ---------------------------------------------------------------- folder browser

interface ListResult {
  path: string
  parent: string | null
  root: string
  entries: Array<{ name: string; path: string; isDir: boolean; isProject: boolean }>
}

export async function pickServerFolder(opts: { title?: string; multiple?: boolean } = {}): Promise<string | null> {
  ensureStyles()
  const cfg = await getWebConfig()
  return new Promise((resolve) => {
    const overlay = document.createElement("div")
    overlay.className = "lw-overlay"
    const modal = document.createElement("div")
    modal.className = "lw-modal"
    modal.setAttribute("role", "dialog")
    overlay.appendChild(modal)

    const head = document.createElement("div")
    head.className = "lw-head"
    head.textContent = opts.title || T.chooseFolder
    const pathEl = document.createElement("div")
    pathEl.className = "lw-path"
    const list = document.createElement("div")
    list.className = "lw-list"
    const hint = document.createElement("div")
    hint.className = "lw-hint"
    hint.textContent = `${T.hint} (${cfg.dataDir})`
    const foot = document.createElement("div")
    foot.className = "lw-foot"
    modal.append(head, pathEl, list, hint, foot)

    const mkBtn = (label: string, cls = "") => {
      const b = document.createElement("button")
      b.type = "button"
      b.className = `lw-btn ${cls}`
      b.textContent = label
      return b
    }
    const upBtn = mkBtn(`↑ ${T.up}`)
    const newBtn = mkBtn(T.newFolder)
    const uploadBtn = mkBtn(T.uploadFolder)
    const spacer = document.createElement("span")
    spacer.className = "lw-spacer"
    const cancelBtn = mkBtn(T.cancel)
    const selectBtn = mkBtn(T.select, "lw-primary")
    foot.append(upBtn, newBtn, uploadBtn, spacer, cancelBtn, selectBtn)

    let current: ListResult | null = null
    const finish = (value: string | null) => {
      overlay.remove()
      document.removeEventListener("keydown", onKey, true)
      resolve(value)
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation()
        finish(null)
      }
    }
    document.addEventListener("keydown", onKey, true)

    const load = async (path?: string) => {
      try {
        const q = path ? `?path=${encodeURIComponent(path)}` : ""
        current = await apiJson<ListResult>(`/api/fs/list${q}`, undefined, "GET")
      } catch (err) {
        list.innerHTML = ""
        const e = document.createElement("div")
        e.className = "lw-empty"
        e.textContent = String(err)
        list.appendChild(e)
        return
      }
      pathEl.textContent = current.path
      upBtn.disabled = !current.parent
      list.innerHTML = ""
      if (current.entries.length === 0) {
        const e = document.createElement("div")
        e.className = "lw-empty"
        e.textContent = T.empty
        list.appendChild(e)
      }
      for (const entry of current.entries) {
        const row = document.createElement("div")
        row.className = "lw-item"
        row.textContent = `📁 ${entry.name}`
        if (entry.isProject) {
          const badge = document.createElement("span")
          badge.className = "lw-badge"
          badge.textContent = T.project
          row.appendChild(badge)
        }
        row.addEventListener("click", () => void load(entry.path))
        row.addEventListener("dblclick", () => finish(entry.path))
        list.appendChild(row)
      }
    }

    upBtn.addEventListener("click", () => current?.parent && void load(current.parent))
    newBtn.addEventListener("click", async () => {
      if (!current) return
      const name = window.prompt(T.newFolderPrompt)?.trim()
      if (!name || /[\\/]/.test(name) || name === "." || name === "..") return
      try {
        const res = await apiJson<{ path: string }>("/api/fs/mkdir", { path: `${current.path}/${name}` })
        await load(res.path)
      } catch (err) {
        window.alert(String(err))
      }
    })
    uploadBtn.addEventListener("click", async () => {
      const files = await pickFiles({ multiple: true, directory: true })
      if (!files || files.length === 0) return
      try {
        const res = await uploadFiles(files)
        if (res) finish(res.dirs[0] ?? res.root)
      } catch (err) {
        window.alert(String(err))
      }
    })
    cancelBtn.addEventListener("click", () => finish(null))
    selectBtn.addEventListener("click", () => current && finish(current.path))
    overlay.addEventListener("mousedown", (e) => {
      if (e.target === overlay) finish(null)
    })

    document.body.appendChild(overlay)
    void load(cfg.projectsDir)
  })
}

// ---------------------------------------------------------------- sign out

async function signOut(): Promise<void> {
  try {
    await apiFetch("/api/auth/logout", { method: "POST" })
  } finally {
    location.href = "/login"
  }
}

/**
 * Account actions live in the left icon bar (web build only); it asks for
 * them with window events so the shared component needs no web import.
 */
function installAccountActions(): void {
  if (typeof window === "undefined") return
  window.addEventListener("lw:sign-out", () => void signOut())
  window.addEventListener("lw:change-password", () => openChangePassword())
}

function openChangePassword(): void {
  ensureStyles()
  const overlay = document.createElement("div")
  overlay.className = "lw-overlay"
  const modal = document.createElement("div")
  modal.className = "lw-modal"
  modal.style.width = "min(380px,94vw)"
  const head = document.createElement("div")
  head.className = "lw-head"
  head.textContent = T.changePw
  const form = document.createElement("form")
  form.className = "lw-form"
  const mk = (ph: string, ac: string) => {
    const i = document.createElement("input")
    i.type = "password"
    i.placeholder = ph
    i.setAttribute("autocomplete", ac)
    i.required = true
    form.appendChild(i)
    return i
  }
  const cur = mk(T.curPw, "current-password")
  const nw = mk(T.newPw, "new-password")
  const cf = mk(T.confirmPw, "new-password")
  const err = document.createElement("div")
  err.className = "lw-err"
  form.appendChild(err)
  const foot = document.createElement("div")
  foot.className = "lw-foot"
  const sp = document.createElement("span")
  sp.className = "lw-spacer"
  const cancel = document.createElement("button")
  cancel.type = "button"
  cancel.className = "lw-btn"
  cancel.textContent = T.cancel
  const ok = document.createElement("button")
  ok.type = "submit"
  ok.className = "lw-btn lw-primary"
  ok.textContent = T.save
  foot.append(sp, cancel, ok)
  form.appendChild(foot)
  modal.append(head, form)
  overlay.appendChild(modal)
  const close = () => overlay.remove()
  cancel.addEventListener("click", close)
  overlay.addEventListener("click", (e) => { if (e.target === overlay) close() })
  form.addEventListener("submit", async (e) => {
    e.preventDefault()
    err.textContent = ""
    if (nw.value.length < 8) { err.textContent = T.pwShort; return }
    if (nw.value !== cf.value) { err.textContent = T.pwMismatch; return }
    ok.disabled = true
    try {
      const res = await apiFetch("/api/auth/password", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ current: cur.value, new: nw.value }),
      })
      if (!res.ok) {
        let msg = `${res.status}`
        try { const j = await res.json(); msg = j.error || msg } catch { /* ignore */ }
        err.textContent = msg
        return
      }
      close()
      alert(T.pwDone)
    } catch (ex) {
      err.textContent = String(ex)
    } finally {
      ok.disabled = false
    }
  })
  document.body.appendChild(overlay)
  cur.focus()
}

// ---------------------------------------------------------------- localhost services

/**
 * The desktop UI talks to its own clip server (127.0.0.1:19827) and REST
 * API (127.0.0.1:19828) with plain fetch(). In the browser those would hit
 * the *visitor's* machine, so route them through the authenticated server.
 */
function installLocalServiceRewrite(): void {
  if (typeof window === "undefined" || (window as unknown as { __lwFetchPatched?: boolean }).__lwFetchPatched) return
  ;(window as unknown as { __lwFetchPatched?: boolean }).__lwFetchPatched = true
  const map: Array<[RegExp, string]> = [
    [/^https?:\/\/(?:127\.0\.0\.1|localhost):19827\//, "/api/clip/"],
    [/^https?:\/\/(?:127\.0\.0\.1|localhost):19828\//, "/api/local-api/"],
  ]
  window.fetch = (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url
    for (const [re, prefix] of map) {
      if (re.test(url)) {
        const rewritten = url.replace(re, prefix)
        const headers = new Headers(init?.headers ?? (input instanceof Request ? input.headers : undefined))
        headers.set("X-LLM-Wiki", "1")
        return nativeFetch(rewritten, { ...init, headers, credentials: "same-origin" })
      }
    }
    return nativeFetch(input as RequestInfo, init)
  }
}

installLocalServiceRewrite()
installAccountActions()
