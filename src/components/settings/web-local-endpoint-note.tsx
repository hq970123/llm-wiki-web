import { useTranslation } from "react-i18next"
import { IS_WEB_BUILD } from "@/lib/api-server-constants"

/**
 * Web build only: endpoints are fetched by the server, so "localhost" or a
 * LAN address points at the server itself, not the visitor's computer, and
 * the server's proxy refuses private addresses by default.
 */
export function isLocalOrPrivateEndpoint(value: string | undefined | null): boolean {
  const raw = String(value ?? "").trim()
  if (!raw) return false
  let host: string
  try {
    host = new URL(/^[a-z]+:\/\//i.test(raw) ? raw : `http://${raw}`).hostname.toLowerCase()
  } catch {
    return false
  }
  host = host.replace(/^\[|\]$/g, "")
  if (host === "localhost" || host.endsWith(".localhost") || host.endsWith(".local")) return true
  if (host === "::1" || host.startsWith("fc") || host.startsWith("fd") || host.startsWith("fe80")) return true
  const m = host.match(/^(\d+)\.(\d+)\.(\d+)\.(\d+)$/)
  if (!m) return false
  const [a, b] = [Number(m[1]), Number(m[2])]
  return (
    a === 127 || a === 10 || a === 0 ||
    (a === 172 && b >= 16 && b <= 31) ||
    (a === 192 && b === 168) ||
    (a === 169 && b === 254) ||
    (a === 100 && b >= 64 && b <= 127)
  )
}

export function WebLocalEndpointNote({ url }: { url: string | undefined | null }) {
  const { t } = useTranslation()
  if (!IS_WEB_BUILD || !isLocalOrPrivateEndpoint(url)) return null
  return (
    <p className="text-xs leading-relaxed text-amber-700 dark:text-amber-400">
      {t("webUi.localEndpointNote")}
    </p>
  )
}
