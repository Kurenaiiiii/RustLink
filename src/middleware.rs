use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::{
    body::Body,
    extract::State,
    http::{HeaderValue, Request, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
    Router,
};
use tower_http::{
    compression::CompressionLayer,
    cors::CorsLayer,
    limit::RequestBodyLimitLayer,
    set_header::SetResponseHeaderLayer,
};

use crate::config::NodeLinkConfig;
use crate::state::SharedState;

async fn request_logger(request: Request<Body>, next: Next) -> Response {
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(|q| format!("?{}", q)).unwrap_or_default();
    let user_agent = request
        .headers()
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");
    let remote = request
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");
    tracing::info!(target: "Request", "[{remote}] {method} - {path}{query} | {user_agent}");
    next.run(request).await
}

pub fn apply_middleware(router: Router, config: &NodeLinkConfig, state: Option<SharedState>) -> Router {
    let mut router = router;

    // Request logging
    router = router.layer(axum::middleware::from_fn(request_logger));

    // Add NodeLink response headers
    let nodlink_api_version = HeaderValue::from_static("4");
    let iam_nodelink = HeaderValue::from_static("true");
    router = router
        .layer(SetResponseHeaderLayer::overriding(
            "Nodelink-Api-Version".parse::<axum::http::HeaderName>().unwrap(),
            nodlink_api_version,
        ))
        .layer(SetResponseHeaderLayer::overriding(
            "IamNodelink".parse::<axum::http::HeaderName>().unwrap(),
            iam_nodelink,
        ));

    // Compression (NodeLink order: zstd > br > gzip > deflate)
    router = router.layer(CompressionLayer::new());

    // CORS — permissive for profiler UI and WebSocket access
    router = router.layer(CorsLayer::permissive());

    // Plugin REST hook middleware (mirrors NodeLink's onRESTRequest)
    if let Some(state) = state.clone() {
        router = router.layer(axum::middleware::from_fn_with_state(
            state,
            plugin_rest_hook_middleware,
        ));
    }

    if config.dos_protection.enabled {
        router = router.layer(RequestBodyLimitLayer::new(
            config.dos_protection.max_body_size,
        ));
    }

    if config.rate_limit.enabled {
        let limiter = RateLimiter::new(
            config.rate_limit.max_requests,
            Duration::from_millis(config.rate_limit.window_ms),
        );
        router = router.layer(axum::middleware::from_fn_with_state(
            limiter,
            rate_limit_middleware,
        ));
    }

    router
}

async fn plugin_rest_hook_middleware(
    State(state): State<SharedState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let headers: serde_json::Value = request.headers().iter()
        .map(|(k, v)| {
            (k.to_string(), serde_json::Value::String(v.to_str().unwrap_or("").to_string()))
        })
        .collect::<serde_json::Map<_, _>>()
        .into();

    // Fire-and-forget: do not block request processing on plugin hooks
    let pm = state.plugin_manager.clone();
    tokio::spawn(async move {
        pm.on_rest_request(&method, &path, &headers).await;
    });

    next.run(request).await
}

#[derive(Clone)]
struct RateLimiter {
    inner: std::sync::Arc<Mutex<HashMap<String, (u32, Instant)>>>,
    max_requests: u32,
    window: Duration,
}

impl RateLimiter {
    fn new(max_requests: u32, window: Duration) -> Self {
        Self {
            inner: std::sync::Arc::new(Mutex::new(HashMap::new())),
            max_requests,
            window,
        }
    }

    fn get_headers(&self, key: &str) -> (u32, u32, u64) {
        let map = self.inner.lock().unwrap();
        let now = Instant::now();
        if let Some((count, start)) = map.get(key) {
            let remaining = self.max_requests.saturating_sub(*count);
            let reset = self.window.as_secs().saturating_sub(now.duration_since(*start).as_secs());
            (self.max_requests, remaining, reset)
        } else {
            (self.max_requests, self.max_requests, 0)
        }
    }

    fn check(&self, key: &str) -> bool {
        let mut map = self.inner.lock().unwrap();
        let now = Instant::now();

        map.retain(|_, (_, start)| now.duration_since(*start) <= self.window);

        if let Some((count, start)) = map.get_mut(key) {
            if now.duration_since(*start) > self.window {
                *count = 1;
                *start = now;
                true
            } else if *count >= self.max_requests {
                false
            } else {
                *count += 1;
                true
            }
        } else {
            map.insert(key.to_string(), (1, now));
            true
        }
    }
}

async fn rate_limit_middleware(
    State(limiter): State<RateLimiter>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let client_key = request
        .headers()
        .get(header::FORWARDED)
        .and_then(|v| v.to_str().ok())
        .or_else(|| {
            request
                .headers()
                .get("X-Forwarded-For")
                .and_then(|v| v.to_str().ok())
        })
        .or_else(|| {
            request
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
        })
        .unwrap_or("global")
        .to_string();

    let (limit, remaining, reset) = limiter.get_headers(&client_key);

    if !limiter.check(&client_key) {
        let mut resp = (StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded").into_response();
        let headers = resp.headers_mut();
        headers.insert("X-RateLimit-Limit", limit.to_string().parse().unwrap());
        headers.insert("X-RateLimit-Remaining", "0".parse().unwrap());
        headers.insert("X-RateLimit-Reset", reset.to_string().parse().unwrap());
        return resp;
    }

    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert("X-RateLimit-Limit", limit.to_string().parse().unwrap());
    headers.insert("X-RateLimit-Remaining", remaining.to_string().parse().unwrap());
    headers.insert("X-RateLimit-Reset", reset.to_string().parse().unwrap());
    response
}
