use axum::{
    body::Body,
    extract::{OriginalUri, State},
    http::{HeaderMap, Method, StatusCode},
    response::Response,
    routing::any,
    Router,
};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;
use std::{net::SocketAddr, time::Instant};
use tracing::info;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt};
use url::Url;

#[derive(Clone)]
struct AppState {
    upstream: Url,
    client: reqwest::Client,
}

#[derive(Serialize, Default)]
struct CranInfo {
    artifact_type: &'static str,
    package: Option<String>,
    version: Option<String>,
    r_minor: Option<String>,
    os: Option<String>,
}

static RE_SRC: Lazy<Regex> = Lazy::new(|| Regex::new(r"^/src/contrib/([A-Za-z0-9.]+)_([^/]+)\.tar\.gz$").unwrap());
static RE_ARCH: Lazy<Regex> = Lazy::new(|| Regex::new(r"^/src/contrib/Archive/([^/]+)/[^/]+_([^/]+)\.tar\.gz$").unwrap());
static RE_WIN:  Lazy<Regex> = Lazy::new(|| Regex::new(r"^/bin/windows/contrib/(\d+\.\d+)/([^_]+)_([^/]+)\.zip$").unwrap());
static RE_MAC:  Lazy<Regex> = Lazy::new(|| Regex::new(r"^/bin/macosx/.*/contrib/(\d+\.\d+)/([^_]+)_([^/]+)\.tgz$").unwrap());
static RE_IDX_TXT: Lazy<Regex> = Lazy::new(|| Regex::new(r"^/(src/contrib|bin/.*/contrib/\d+\.\d+)/PACKAGES$").unwrap());
static RE_IDX_GZ:  Lazy<Regex> = Lazy::new(|| Regex::new(r"^/(src/contrib|bin/.*/contrib/\d+\.\d+)/PACKAGES\.gz$").unwrap());
static RE_IDX_RDS: Lazy<Regex> = Lazy::new(|| Regex::new(r"^/(src/contrib|bin/.*/contrib/\d+\.\d+)/PACKAGES\.rds$").unwrap());

fn parse_cran(path: &str) -> CranInfo {
    if let Some(c) = RE_SRC.captures(path) {
        return CranInfo { artifact_type: "src_tar", package: Some(c[1].into()), version: Some(c[2].into()), ..Default::default() };
    }
    if let Some(c) = RE_ARCH.captures(path) {
        return CranInfo { artifact_type: "archive_tar", package: Some(c[1].into()), version: Some(c[2].into()), ..Default::default() };
    }
    if let Some(c) = RE_WIN.captures(path) {
        return CranInfo { artifact_type: "win_zip", r_minor: Some(c[1].into()), package: Some(c[2].into()), version: Some(c[3].into()), os: Some("windows".into()) };
    }
    if let Some(c) = RE_MAC.captures(path) {
        return CranInfo { artifact_type: "mac_tgz", r_minor: Some(c[1].into()), package: Some(c[2].into()), version: Some(c[3].into()), os: Some("macos".into()) };
    }
    if RE_IDX_RDS.is_match(path) { return CranInfo { artifact_type: "index_rds", ..Default::default() }; }
    if RE_IDX_GZ.is_match(path)  { return CranInfo { artifact_type: "index_gz",  ..Default::default() }; }
    if RE_IDX_TXT.is_match(path) { return CranInfo { artifact_type: "index_text",..Default::default() }; }
    CranInfo { artifact_type: "unknown", ..Default::default() }
}

// Remove hop-by-hop and sensitive headers; let reqwest set correct Host
fn strip_hop_headers(headers: &mut HeaderMap) {
    for name in ["connection","proxy-connection","keep-alive","transfer-encoding","te","upgrade","trailer","host"] {
        headers.remove(name);
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // JSON logs by default; configure with RUST_LOG=info or debug
    tracing_subscriber::registry()
        .with(fmt::layer().json().with_target(false).with_level(true))
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let upstream = std::env::var("UPSTREAM_BASE")
        .unwrap_or_else(|_| "https://cloud.r-project.org".to_string());
    let upstream = Url::parse(&upstream)?;

    let client = reqwest::Client::builder()
        .http2_adaptive_window(true)
        .pool_max_idle_per_host(8)
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(60))
        .use_rustls_tls()
        .build()?;

    let state = AppState { upstream, client };

    let app = Router::new()
        .route("/healthz", any(|| async { "ok" }))
        .fallback(any(proxy))
        .with_state(state);

    let addr: SocketAddr = std::env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".into())
        .parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn proxy(
    State(state): State<AppState>,
    method: Method,
    mut headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    body: Body,
) -> Result<Response, (StatusCode, String)> {
    // Build target URL from base + path/query
    let mut target = state.upstream.clone();
    target.set_path(uri.path());
    target.set_query(uri.query());

    // Prepare outgoing request
    let mut req = state.client.request(method.clone(), target);
    {
        // Copy headers minus hop-by-hop; let reqwest compute Host
        strip_hop_headers(&mut headers);
        req = req.headers(headers.clone());
    }

    // Proxy body only when not GET/HEAD (rare for CRAN)
    let req = if matches!(method, Method::GET | Method::HEAD) {
        req
    } else {
        use http_body_util::BodyExt;
        let bytes = body.collect().await
            .map_err(|_| (StatusCode::BAD_REQUEST, "invalid request body".into()))?
            .to_bytes();
        req.body(bytes)
    };

    // Start timer & collect some request metadata
    let started = std::time::Instant::now();
    let ua = headers.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("-");
    let range = headers.get("range").and_then(|v| v.to_str().ok()).unwrap_or("-");
    let path = uri.path();
    let derived = parse_cran(path);

    // Send to upstream
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error=%e, %path, %ua, "upstream_error");
            return Err((StatusCode::BAD_GATEWAY, "upstream error".into()));
        }
    };

    let status = resp.status();
    let etag_out = resp.headers().get("etag").and_then(|v| v.to_str().ok()).unwrap_or("-");
    let content_length = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).unwrap_or("-");

    // Log one structured line
    info!(
        target: "cran",
        path=%path,
        status=%status.as_u16(),
        latency_ms=%started.elapsed().as_millis(),
        ua=%ua,
        range=%range,
        etag_out=%etag_out,
        content_length=%content_length,
        derived=%serde_json::to_string(&derived).unwrap(),
        "proxied"
    );

    // Build response back to client
    let mut builder = Response::builder().status(status);
    // copy response headers, stripping hop-by-hop
    let mut out_headers = HeaderMap::new();
    for (k, v) in resp.headers().iter() {
        out_headers.insert(k, v.clone());
    }
    strip_hop_headers(&mut out_headers);
    *builder.headers_mut().unwrap() = out_headers;

    // Stream body back
    use futures_util::TryStreamExt;
    let stream = resp.bytes_stream().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
    let body = axum::body::Body::from_stream(stream);
    Ok(builder.body(body).unwrap())
}
