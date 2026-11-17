//! Outbound HTTP forwarding.
//!
//! `ANY /api/proxy` replaces tauri-plugin-http's Rust-side `fetch`: the
//! browser shim sends the original method + body, with the target URL in
//! `X-Proxy-Url` and the original request headers as base64(JSON) in
//! `X-Proxy-Headers`. The upstream response (status, headers, body) is
//! streamed straight back, so SSE LLM streaming keeps working.
//!
//! SSRF guard: targets resolving to loopback / private / link-local
//! addresses are refused unless `LLM_WIKI_PROXY_ALLOW_PRIVATE=1` (needed
//! for e.g. a local Ollama). DNS is resolved once and pinned for the
//! request to prevent rebinding.

use std::net::{IpAddr, SocketAddr};

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine;
use futures::TryStreamExt;
use serde_json::{json, Value};

use super::AppState;

fn proxy_error(code: StatusCode, msg: String) -> Response {
    let mut res = (code, Json(json!({ "error": msg }))).into_response();
    res.headers_mut()
        .insert("x-proxy-error", HeaderValue::from_static("1"));
    res
}

pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xC0) == 64) // CGNAT 100.64/10
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_ip(IpAddr::V4(v4));
            }
            let seg = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (seg[0] & 0xfe00) == 0xfc00 // unique local
                || (seg[0] & 0xffc0) == 0xfe80 // link local
        }
    }
}

const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
];

fn decode_headers(h: &HeaderMap) -> Result<Vec<(String, String)>, String> {
    let Some(raw) = h.get("x-proxy-headers") else {
        return Ok(Vec::new());
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw.as_bytes())
        .map_err(|e| format!("bad X-Proxy-Headers: {e}"))?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| format!("bad X-Proxy-Headers: {e}"))?;
    let mut out = Vec::new();
    match v {
        Value::Object(map) => {
            for (k, v) in map {
                if let Some(s) = v.as_str() {
                    out.push((k, s.to_string()));
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                if let (Some(k), Some(v)) = (
                    item.get(0).and_then(Value::as_str),
                    item.get(1).and_then(Value::as_str),
                ) {
                    out.push((k.to_string(), v.to_string()));
                }
            }
        }
        _ => {}
    }
    Ok(out)
}

pub async fn handle_proxy(State(st): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let Some(url_str) = parts
        .headers
        .get("x-proxy-url")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
    else {
        return proxy_error(StatusCode::BAD_REQUEST, "missing X-Proxy-Url".into());
    };
    let url = match url::Url::parse(&url_str) {
        Ok(u) => u,
        Err(e) => return proxy_error(StatusCode::BAD_REQUEST, format!("invalid url: {e}")),
    };
    if !matches!(url.scheme(), "http" | "https") {
        return proxy_error(StatusCode::BAD_REQUEST, "only http(s) URLs can be fetched".into());
    }
    let Some(host) = url.host_str().map(|h| h.trim_matches(|c| c == '[' || c == ']').to_string()) else {
        return proxy_error(StatusCode::BAD_REQUEST, "url has no host".into());
    };
    let port = url.port_or_known_default().unwrap_or(443);

    // Resolve + SSRF check.
    let addrs: Vec<SocketAddr> = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        match tokio::net::lookup_host((host.as_str(), port)).await {
            Ok(it) => it.collect(),
            Err(e) => {
                return proxy_error(StatusCode::BAD_GATEWAY, format!("DNS lookup failed for {host}: {e}"))
            }
        }
    };
    if addrs.is_empty() {
        return proxy_error(StatusCode::BAD_GATEWAY, format!("no address for {host}"));
    }
    if !st.cfg.allow_private_proxy && addrs.iter().any(|a| is_private_ip(a.ip())) {
        return proxy_error(
            StatusCode::FORBIDDEN,
            format!("Refusing to fetch {host}: it resolves to a private/loopback address (set LLM_WIKI_PROXY_ALLOW_PRIVATE=1 to allow, e.g. for a local Ollama)"),
        );
    }

    let headers = match decode_headers(&parts.headers) {
        Ok(h) => h,
        Err(e) => return proxy_error(StatusCode::BAD_REQUEST, e),
    };

    let mut builder = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none()); // redirects are followed by the browser shim (re-checked here)
    if host.parse::<IpAddr>().is_err() {
        builder = builder.resolve_to_addrs(&host, &addrs);
    }
    builder = crate::proxy::configure_http_client(builder);
    let client = match builder.build() {
        Ok(c) => c,
        Err(e) => return proxy_error(StatusCode::INTERNAL_SERVER_ERROR, format!("client: {e}")),
    };

    let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);
    let mut rb = client.request(method.clone(), url);
    for (k, v) in headers {
        let lk = k.to_ascii_lowercase();
        if HOP_BY_HOP.contains(&lk.as_str())
            || lk == "accept-encoding"
            || lk == "cookie"
            || lk == "origin"
            || lk == "referer"
        {
            continue;
        }
        rb = rb.header(k, v);
    }
    if method != reqwest::Method::GET && method != reqwest::Method::HEAD {
        let stream = body.into_data_stream().map_err(|e| std::io::Error::other(e.to_string()));
        rb = rb.body(reqwest::Body::wrap_stream(stream));
    }

    let upstream = match rb.send().await {
        Ok(r) => r,
        Err(e) => {
            let code = if e.is_timeout() { StatusCode::GATEWAY_TIMEOUT } else { StatusCode::BAD_GATEWAY };
            return proxy_error(code, format!("upstream request failed: {e}"));
        }
    };
    stream_back(upstream, true)
}

fn stream_back(upstream: reqwest::Response, mark_final_url: bool) -> Response {
    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut headers = HeaderMap::new();
    for (k, v) in upstream.headers() {
        let lk = k.as_str().to_ascii_lowercase();
        if HOP_BY_HOP.contains(&lk.as_str()) || lk == "set-cookie" || lk.starts_with("access-control-") {
            continue;
        }
        if let (Ok(n), Ok(val)) = (HeaderName::from_bytes(k.as_str().as_bytes()), HeaderValue::from_bytes(v.as_bytes())) {
            headers.append(n, val);
        }
    }
    if mark_final_url {
        if let Ok(v) = HeaderValue::from_str(upstream.url().as_str()) {
            headers.insert("x-proxy-final-url", v);
        }
    }
    // Disable proxy buffering (nginx etc.) so streamed tokens arrive live.
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    let stream = upstream.bytes_stream().map_err(std::io::Error::other);
    let mut res = Response::new(Body::from_stream(stream));
    if mark_final_url {
        // /api/proxy: the real upstream status travels in a header so the
        // browser never acts on it itself (e.g. auto-following redirects,
        // which would bypass the SSRF check, or opaque redirect responses).
        headers.insert("x-proxy-status", HeaderValue::from(status.as_u16()));
        *res.status_mut() = StatusCode::OK;
    } else {
        *res.status_mut() = status;
    }
    *res.headers_mut() = headers;
    res
}

async fn forward_local(port: u16, rest: &str, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let query = parts.uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    let url = format!("http://127.0.0.1:{port}/{rest}{query}");
    let client = match reqwest::Client::builder().no_proxy().build() {
        Ok(c) => c,
        Err(e) => return proxy_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);
    let mut rb = client.request(method, url);
    for name in ["content-type", "accept", "authorization", "x-llm-wiki-token"] {
        if let Some(v) = parts.headers.get(name) {
            rb = rb.header(name, v.as_bytes());
        }
    }
    let bytes = match axum::body::to_bytes(body, 64 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => return proxy_error(StatusCode::BAD_REQUEST, e.to_string()),
    };
    if !bytes.is_empty() {
        rb = rb.body(bytes);
    }
    match rb.send().await {
        Ok(r) => stream_back(r, false),
        Err(e) => proxy_error(StatusCode::BAD_GATEWAY, format!("internal service on port {port} unavailable: {e}")),
    }
}

pub async fn handle_clip_forward(State(st): State<AppState>, Path(rest): Path<String>, req: Request) -> Response {
    if !st.cfg.enable_clip_server {
        return proxy_error(StatusCode::SERVICE_UNAVAILABLE, "clip server disabled".into());
    }
    forward_local(19827, &rest, req).await
}

pub async fn handle_local_api_forward(State(st): State<AppState>, Path(rest): Path<String>, req: Request) -> Response {
    if !st.cfg.enable_api_server {
        return proxy_error(StatusCode::SERVICE_UNAVAILABLE, "local API server disabled (LLM_WIKI_ENABLE_API_SERVER=1)".into());
    }
    forward_local(19828, &rest, req).await
}
