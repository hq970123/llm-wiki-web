declare const __LLM_WIKI_WEB__: boolean | undefined

export const API_SERVER_PORT = 19828

/** True in the self-hosted web build (vite --mode web). */
export const IS_WEB_BUILD =
  typeof __LLM_WIKI_WEB__ !== "undefined" && __LLM_WIKI_WEB__ === true

/**
 * Desktop: the loopback API. Web: the site's own origin — the server
 * exposes the API at <origin>/api/v1/* and MCP (streamable HTTP) at
 * <origin>/mcp, so external clients use the domain directly.
 */
export const API_SERVER_BASE_URL =
  IS_WEB_BUILD && typeof window !== "undefined" && window.location?.origin
    ? window.location.origin
    : `http://127.0.0.1:${API_SERVER_PORT}`
export const API_SERVER_HEALTH_URL = `${API_SERVER_BASE_URL}/api/v1/health`
export const MCP_HTTP_URL = `${API_SERVER_BASE_URL}/mcp`
