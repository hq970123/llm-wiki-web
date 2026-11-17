/**
 * Web shim for `@tauri-apps/plugin-http`: requests go through the
 * server's /api/proxy so CORS-hostile LLM endpoints keep working.
 * Responses (incl. SSE token streams) are streamed back unchanged.
 */
import { WEB_HEADER, nativeFetch } from "./runtime"

export type ClientOptions = { danger?: unknown; proxy?: unknown; connectTimeout?: number; maxRedirections?: number }

function encodeHeaders(headers: Headers): string {
  const pairs: Array<[string, string]> = []
  headers.forEach((v, k) => pairs.push([k, v]))
  const json = JSON.stringify(pairs)
  const bytes = new TextEncoder().encode(json)
  let bin = ""
  for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i])
  return btoa(bin)
}

function networkError(message: string): TypeError {
  // Same shape WebKit/Chromium use, so isFetchNetworkError() recognises it.
  return new TypeError(`Failed to fetch: ${message}`)
}

export async function fetch(input: URL | Request | string, init?: RequestInit & ClientOptions): Promise<Response> {
  const req = input instanceof Request ? input : new Request(String(input), init)
  let url = req.url
  let method = (init?.method ?? req.method ?? "GET").toUpperCase()
  const headers = new Headers(init?.headers ?? req.headers)
  let body: BodyInit | null = null
  if (method !== "GET" && method !== "HEAD") {
    if (init?.body !== undefined && init.body !== null) body = init.body as BodyInit
    else if (input instanceof Request) body = await input.arrayBuffer()
    // A ReadableStream body needs duplex streaming; buffer it instead.
    if (body instanceof ReadableStream) body = await new Response(body).arrayBuffer()
  }
  if (body instanceof FormData || body instanceof URLSearchParams) {
    // Let the browser serialise it, then forward the exact bytes + type.
    const tmp = new Request("http://localhost/", { method: "POST", body })
    const ct = tmp.headers.get("content-type")
    if (ct) headers.set("content-type", ct)
    body = await tmp.arrayBuffer()
  }
  const signal = init?.signal ?? (input instanceof Request ? input.signal : undefined)

  for (let hop = 0; hop < 6; hop++) {
    let res: Response
    try {
      res = await nativeFetch("/api/proxy", {
        method,
        headers: { ...WEB_HEADER, "X-Proxy-Url": url, "X-Proxy-Headers": encodeHeaders(headers) },
        body,
        signal,
        credentials: "same-origin",
      })
    } catch (err) {
      if (err instanceof DOMException && err.name === "AbortError") throw err
      throw networkError(String(err))
    }
    if (res.headers.get("x-proxy-error") === "1") {
      let msg = `HTTP ${res.status}`
      try {
        msg = (await res.json()).error ?? msg
      } catch { /* ignore */ }
      if (res.status === 401) location.href = "/login"
      throw networkError(msg)
    }
    const status = Number(res.headers.get("x-proxy-status") ?? res.status)
    const location_ = res.headers.get("location")
    if ([301, 302, 303, 307, 308].includes(status) && location_ && (init?.redirect ?? "follow") === "follow") {
      void res.body?.cancel()
      url = new URL(location_, url).toString()
      if (status === 303 || ((status === 301 || status === 302) && method === "POST")) {
        method = "GET"
        body = null
      }
      continue
    }
    const outHeaders = new Headers()
    res.headers.forEach((v, k) => {
      if (!k.startsWith("x-proxy-")) outHeaders.append(k, v)
    })
    const nullBody = status === 204 || status === 205 || status === 304 || method === "HEAD"
    const out = new Response(nullBody ? null : res.body, {
      status: status >= 200 && status <= 599 ? status : 502,
      statusText: "",
      headers: outHeaders,
    })
    Object.defineProperty(out, "url", { value: res.headers.get("x-proxy-final-url") ?? url })
    return out
  }
  throw networkError("too many redirects")
}
