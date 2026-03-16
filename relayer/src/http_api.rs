//! Minimal HTTP API server for the InterLink relayer (Phase 5 / Phase 10).
//!
//! Provides the gas estimation and competitive comparison endpoints called out
//! in the checklist, plus Prometheus metrics scraping for Grafana (Phase 10).
//!
//! Implemented over raw tokio TCP — no additional web-framework dependency.
//! For production, place an nginx/Caddy reverse proxy in front for TLS.
//!
//! # Routes
//!
//! | Method | Path              | Description                                    |
//! |--------|-------------------|------------------------------------------------|
//! | GET    | /health           | Liveness check — returns `{"status":"ok"}`     |
//! | GET    | /quote            | Gas + fee estimate for a transfer              |
//! | GET    | /compare          | Competitor cost comparison table               |
//! | GET    | /metrics          | Prometheus text-format scrape endpoint         |
//! | GET    | /metrics/json     | JSON metrics snapshot                          |
//!
//! # Quote query parameters
//!
//! - `amount`     — token amount in wei (default: 1e18 = 1 ETH)
//! - `usd_cents`  — USD value of amount in cents (default: 300 000 = $3 000)
//! - `gas_gwei`   — source chain gas price in gwei (default: 30)
//! - `batch_size` — current batch size for proof amortisation (default: 100)
//! - `eth_usd`    — ETH price in USD for cost conversion (default: 3 000)
//!
//! # Example
//!
//! ```bash
//! curl "http://localhost:8080/health"
//! curl "http://localhost:8080/quote?usd_cents=1000000&gas_gwei=50"
//! curl "http://localhost:8080/compare?usd_cents=500000"
//! curl "http://localhost:8080/metrics"
//! ```

use crate::gas;
use crate::metrics::Metrics;
use crate::webhook::{EventType, WebhookRegistry};
use std::collections::HashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, error, info, warn};

// ─── Server entrypoint ────────────────────────────────────────────────────────

/// Start the HTTP API server on `addr` (e.g. `"0.0.0.0:8080"`).
///
/// Runs until cancelled (drops when the enclosing future is cancelled or the
/// process exits).  Each incoming connection is handled in its own task.
pub async fn serve(addr: &str, metrics: Metrics) {
    serve_with_webhooks(addr, metrics, WebhookRegistry::new()).await;
}

/// Start the HTTP API server with a shared webhook registry.
pub async fn serve_with_webhooks(addr: &str, metrics: Metrics, registry: WebhookRegistry) {
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            error!(addr, error = %e, "HTTP API: failed to bind");
            return;
        }
    };
    info!(addr, "HTTP API server ready (Phase 5/10)");

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                debug!(peer = %peer, "HTTP connection accepted");
                let m = metrics.clone();
                let r = registry.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, m, r).await {
                        warn!(peer = %peer, error = %e, "HTTP connection error");
                    }
                });
            }
            Err(e) => error!(error = %e, "HTTP accept error"),
        }
    }
}

// ─── Connection handler ───────────────────────────────────────────────────────

async fn handle_connection(mut stream: TcpStream, metrics: Metrics, registry: WebhookRegistry) -> Result<(), String> {
    let mut buf = [0u8; 8192]; // generous for any query string
    let n = stream
        .read(&mut buf)
        .await
        .map_err(|e| format!("read: {e}"))?;

    if n == 0 {
        return Ok(());
    }

    let raw = String::from_utf8_lossy(&buf[..n]);
    let (method, path, query, body_str) = parse_request(&raw);

    let (status, content_type, body) = route(method, path, &query, &body_str, &metrics, &registry);

    let response = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         Access-Control-Allow-Origin: *\r\n\
         \r\n\
         {body}",
        status = status,
        reason = reason(status),
        content_type = content_type,
        len = body.len(),
        body = body,
    );

    stream
        .write_all(response.as_bytes())
        .await
        .map_err(|e| format!("write: {e}"))?;

    Ok(())
}

// ─── Router ───────────────────────────────────────────────────────────────────

fn route(
    method: &str,
    path: &str,
    query: &HashMap<String, String>,
    body: &str,
    metrics: &Metrics,
    registry: &WebhookRegistry,
) -> (u16, &'static str, String) {
    match (method, path) {
        ("GET", "/health") => health(),
        ("GET", "/quote") => quote(query),
        ("GET", "/compare") => compare(query),
        ("GET", "/metrics") => prometheus(metrics),
        ("GET", "/metrics/json") => metrics_json(metrics),
        ("GET", "/webhooks") => webhooks_list(registry),
        ("POST", "/webhooks/register") => webhooks_register(registry, body),
        _ if method == "DELETE" && path.starts_with("/webhooks/") => {
            let id = &path["/webhooks/".len()..];
            webhooks_deregister(registry, id)
        }
        _ if method == "GET" && path.starts_with("/webhooks/") => {
            let id = &path["/webhooks/".len()..];
            webhooks_get(registry, id)
        }
        _ => not_found(),
    }
}

// ─── Route handlers ───────────────────────────────────────────────────────────

fn health() -> (u16, &'static str, String) {
    let body = serde_json::json!({
        "status": "ok",
        "service": "interlink-relayer",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime": "ok",
    });
    (200, "application/json", body.to_string())
}

/// GET /quote — gas + fee estimate for a prospective transfer.
///
/// Competitive comparison is embedded so callers can see InterLink vs
/// Wormhole/Stargate/Across in a single API call (Phase 5 gas estimation API).
fn quote(query: &HashMap<String, String>) -> (u16, &'static str, String) {
    let amount: u128 = parse_u128(query, "amount", 1_000_000_000_000_000_000);
    let usd_cents: u64 = parse_u64(query, "usd_cents", 300_000);
    let gas_gwei: u64 = parse_u64(query, "gas_gwei", 30);
    let batch_size: usize = parse_usize(query, "batch_size", 100);
    let eth_usd: u64 = parse_u64(query, "eth_usd", 3_000);

    let est = gas::estimate(amount, usd_cents, gas_gwei, batch_size, eth_usd);
    let cmp = gas::compare(usd_cents);

    let body = serde_json::json!({
        "inputs": {
            "amount_wei": amount.to_string(),
            "usd_cents": usd_cents,
            "gas_price_gwei": gas_gwei,
            "batch_size": batch_size,
            "eth_usd": eth_usd,
        },
        "estimate": {
            "source_gas_units": est.source_gas_units,
            "source_gas_price_gwei": est.source_gas_price_gwei,
            "source_gas_cost_wei": est.source_gas_cost_wei.to_string(),
            "proof_cost_amortised_wei": est.proof_cost_amortised_wei.to_string(),
            "dest_compute_units": est.dest_compute_units,
            "dest_fee_lamports": est.dest_fee_lamports,
            "fee_tier": {
                "name": est.fee_tier.name,
                "bps": est.fee_tier.bps,
                "description": est.fee_tier.description,
            },
            "protocol_fee_amount": est.protocol_fee_amount.to_string(),
        },
        "comparison": {
            "interlink": cmp.interlink,
            "competitors": cmp.competitors,
            "interlink_wins_fee": cmp.interlink_wins_on_fee(),
            "interlink_wins_speed": cmp.interlink_wins_on_speed(),
            "savings_vs_cheapest_cents": cmp.savings_vs_cheapest_cents(),
        },
    });

    (200, "application/json", body.to_string())
}

/// GET /compare — full competitive comparison table.
fn compare(query: &HashMap<String, String>) -> (u16, &'static str, String) {
    let usd_cents: u64 = parse_u64(query, "usd_cents", 100_000);
    let table = gas::format_comparison_table(usd_cents);
    let cmp = gas::compare(usd_cents);

    let body = serde_json::json!({
        "usd_cents": usd_cents,
        "ascii_table": table,
        "data": {
            "interlink": cmp.interlink,
            "competitors": cmp.competitors,
            "interlink_wins_fee": cmp.interlink_wins_on_fee(),
            "interlink_wins_speed": cmp.interlink_wins_on_speed(),
        },
    });
    (200, "application/json", body.to_string())
}

/// GET /metrics — Prometheus text format for Grafana scraping (Phase 10).
fn prometheus(metrics: &Metrics) -> (u16, &'static str, String) {
    (200, "text/plain; version=0.0.4", metrics.prometheus_text())
}

/// GET /metrics/json — JSON metrics snapshot.
fn metrics_json(metrics: &Metrics) -> (u16, &'static str, String) {
    (200, "application/json", metrics.as_json().to_string())
}

/// GET /webhooks — list all registered webhooks.
fn webhooks_list(registry: &WebhookRegistry) -> (u16, &'static str, String) {
    let hooks = registry.list();
    let body = serde_json::json!({
        "count": hooks.len(),
        "active_count": registry.active_count(),
        "webhooks": hooks,
    });
    (200, "application/json", body.to_string())
}

/// POST /webhooks/register — register a new webhook.
///
/// Body: `{ "url": "https://...", "events": ["settlement.complete", "transfer.failed"] }`
/// Use `"events": ["all"]` to receive every event type.
fn webhooks_register(registry: &WebhookRegistry, body: &str) -> (u16, &'static str, String) {
    let parsed: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => {
            return (
                400,
                "application/json",
                r#"{"error":"invalid JSON body"}"#.into(),
            );
        }
    };

    let url = match parsed["url"].as_str() {
        Some(u) if u.starts_with("http://") || u.starts_with("https://") => u.to_string(),
        Some(_) => {
            return (
                400,
                "application/json",
                r#"{"error":"url must start with http:// or https://"}"#.into(),
            );
        }
        None => {
            return (
                400,
                "application/json",
                r#"{"error":"url field required"}"#.into(),
            );
        }
    };

    let events: Vec<EventType> = parsed["events"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| match s {
                    "transfer.initiated" => EventType::TransferInitiated,
                    "finality.confirmed" => EventType::FinalityConfirmed,
                    "proof.generated" => EventType::ProofGenerated,
                    "settlement.complete" => EventType::SettlementComplete,
                    "transfer.failed" => EventType::TransferFailed,
                    _ => EventType::All,
                })
                .collect()
        })
        .unwrap_or_default();

    let reg = registry.register(url, events);
    (201, "application/json", serde_json::to_string(&reg).unwrap_or_default())
}

/// DELETE /webhooks/{id} — deregister a webhook.
fn webhooks_deregister(registry: &WebhookRegistry, id: &str) -> (u16, &'static str, String) {
    if registry.deregister(id) {
        let body = serde_json::json!({ "deleted": true, "id": id });
        (200, "application/json", body.to_string())
    } else {
        let body = serde_json::json!({ "error": "webhook not found", "id": id });
        (404, "application/json", body.to_string())
    }
}

/// GET /webhooks/{id} — fetch a single webhook by ID.
fn webhooks_get(registry: &WebhookRegistry, id: &str) -> (u16, &'static str, String) {
    match registry.get(id) {
        Some(reg) => (200, "application/json", serde_json::to_string(&reg).unwrap_or_default()),
        None => {
            let body = serde_json::json!({ "error": "webhook not found", "id": id });
            (404, "application/json", body.to_string())
        }
    }
}

fn not_found() -> (u16, &'static str, String) {
    let body = serde_json::json!({
        "error": "not found",
        "routes": [
            "/health",
            "/quote",
            "/compare",
            "/metrics",
            "/metrics/json",
            "/webhooks",
            "/webhooks/register (POST)",
            "/webhooks/{id} (GET, DELETE)",
        ],
    });
    (404, "application/json", body.to_string())
}

// ─── HTTP helpers ─────────────────────────────────────────────────────────────

/// Parse an HTTP request into (method, path, query_params, body).
fn parse_request<'a>(
    request: &'a str,
) -> (&'a str, &'a str, HashMap<String, String>, String) {
    let first_line = request.lines().next().unwrap_or("");
    let parts: Vec<&str> = first_line.splitn(3, ' ').collect();
    if parts.len() < 2 {
        return ("", "/", HashMap::new(), String::new());
    }
    let method = parts[0];
    let full_path = parts.get(1).copied().unwrap_or("/");

    let (path, qs) = match full_path.find('?') {
        Some(pos) => (&full_path[..pos], &full_path[pos + 1..]),
        None => (full_path, ""),
    };

    let query = qs
        .split('&')
        .filter(|s| !s.is_empty())
        .filter_map(|pair| {
            let mut kv = pair.splitn(2, '=');
            let k = kv.next()?.to_string();
            let v = kv.next().unwrap_or("").to_string();
            Some((k, v))
        })
        .collect();

    // Extract HTTP body (after the blank line \r\n\r\n separator)
    let body = request
        .find("\r\n\r\n")
        .map(|pos| request[pos + 4..].trim_end_matches('\0').to_string())
        .unwrap_or_default();

    (method, path, query, body)
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Unknown",
    }
}

// ─── Query parameter parsers ──────────────────────────────────────────────────

fn parse_u64(q: &HashMap<String, String>, key: &str, default: u64) -> u64 {
    q.get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn parse_u128(q: &HashMap<String, String>, key: &str, default: u128) -> u128 {
    q.get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn parse_usize(q: &HashMap<String, String>, key: &str, default: usize) -> usize {
    q.get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::Metrics;

    fn make_query(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn test_health_returns_200() {
        let (status, _, body) = health();
        assert_eq!(status, 200);
        assert!(body.contains("ok"));
    }

    #[test]
    fn test_quote_with_defaults() {
        let q = HashMap::new();
        let (status, _, body) = quote(&q);
        assert_eq!(status, 200);
        assert!(body.contains("estimate"));
        assert!(body.contains("comparison"));
        assert!(body.contains("interlink_wins_fee"));
    }

    #[test]
    fn test_quote_with_explicit_params() {
        let q = make_query(&[
            ("usd_cents", "1000000"),  // $10 000
            ("gas_gwei", "50"),
            ("batch_size", "200"),
        ]);
        let (status, _, body) = quote(&q);
        assert_eq!(status, 200);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["inputs"]["usd_cents"], 1_000_000);
        assert_eq!(v["inputs"]["gas_price_gwei"], 50);
    }

    #[test]
    fn test_compare_returns_all_competitors() {
        let q = make_query(&[("usd_cents", "500000")]);
        let (status, _, body) = compare(&q);
        assert_eq!(status, 200);
        assert!(body.contains("Wormhole"));
        assert!(body.contains("Stargate v2"));
        assert!(body.contains("Across"));
    }

    #[test]
    fn test_metrics_prometheus_format() {
        let m = Metrics::new();
        m.record_proof_start();
        m.record_proof_success(80);
        let (status, ct, body) = prometheus(&m);
        assert_eq!(status, 200);
        assert!(ct.contains("text/plain"));
        assert!(body.contains("interlink_proof_gen_total 1"));
    }

    #[test]
    fn test_metrics_json_format() {
        let m = Metrics::new();
        let (status, ct, body) = metrics_json(&m);
        assert_eq!(status, 200);
        assert!(ct.contains("application/json"));
        assert!(body.contains("proof_gen"));
    }

    #[test]
    fn test_not_found_route() {
        use crate::webhook::WebhookRegistry;
        let q = HashMap::new();
        let m = Metrics::new();
        let r = WebhookRegistry::new();
        let (status, _, body) = route("GET", "/nonexistent", &q, "", &m, &r);
        assert_eq!(status, 404);
        assert!(body.contains("routes"));
    }

    #[test]
    fn test_parse_request_no_query() {
        let (method, path, query, body) =
            parse_request("GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert_eq!(method, "GET");
        assert_eq!(path, "/health");
        assert!(query.is_empty());
        assert!(body.is_empty());
    }

    #[test]
    fn test_parse_request_with_query() {
        let (method, path, query, _) =
            parse_request("GET /quote?usd_cents=100000&gas_gwei=30 HTTP/1.1\r\n");
        assert_eq!(method, "GET");
        assert_eq!(path, "/quote");
        assert_eq!(query.get("usd_cents"), Some(&"100000".to_string()));
        assert_eq!(query.get("gas_gwei"), Some(&"30".to_string()));
    }

    #[test]
    fn test_parse_request_with_body() {
        let raw = "POST /webhooks/register HTTP/1.1\r\nContent-Type: application/json\r\n\r\n{\"url\":\"https://example.com\"}";
        let (method, path, _, body) = parse_request(raw);
        assert_eq!(method, "POST");
        assert_eq!(path, "/webhooks/register");
        assert!(body.contains("https://example.com"));
    }

    #[test]
    fn test_webhook_register_and_list() {
        use crate::webhook::WebhookRegistry;
        let registry = WebhookRegistry::new();
        // Register a webhook
        let body = r#"{"url":"https://example.com/hook","events":["settlement.complete"]}"#;
        let (status, _, resp) = webhooks_register(&registry, body);
        assert_eq!(status, 201);
        assert!(resp.contains("https://example.com/hook"));

        // List should show 1
        let (status, _, list_resp) = webhooks_list(&registry);
        assert_eq!(status, 200);
        let v: serde_json::Value = serde_json::from_str(&list_resp).unwrap();
        assert_eq!(v["count"], 1);
        assert_eq!(v["active_count"], 1);

        // Invalid URL should fail
        let bad = r#"{"url":"ftp://bad.com","events":[]}"#;
        let (status2, _, _) = webhooks_register(&registry, bad);
        assert_eq!(status2, 400);

        // Missing URL should fail
        let missing = r#"{"events":["all"]}"#;
        let (status3, _, _) = webhooks_register(&registry, missing);
        assert_eq!(status3, 400);
    }

    #[test]
    fn test_webhook_deregister() {
        use crate::webhook::WebhookRegistry;
        let registry = WebhookRegistry::new();

        let body = r#"{"url":"https://example.com/hook","events":[]}"#;
        let (_, _, resp) = webhooks_register(&registry, body);
        let reg: serde_json::Value = serde_json::from_str(&resp).unwrap();
        let id = reg["id"].as_str().unwrap();

        let (status, _, _) = webhooks_deregister(&registry, id);
        assert_eq!(status, 200);

        let (status, _, _) = webhooks_deregister(&registry, id);
        assert_eq!(status, 404);
    }

    #[test]
    fn test_interlink_wins_tier1_on_quote() {
        // $500 transfer: InterLink is free; Wormhole charges $1+
        let q = make_query(&[("usd_cents", "50000")]); // $500
        let (_, _, body) = quote(&q);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["comparison"]["interlink_wins_fee"], true);
        assert_eq!(v["comparison"]["interlink_wins_speed"], true);
    }

    #[tokio::test]
    async fn test_server_starts_and_accepts() {
        let m = Metrics::new();
        // Bind to a random port to verify the server starts without error.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let addr_str = addr.to_string();
        let m2 = m.clone();
        tokio::spawn(async move {
            // serve() blocks; cancel after test via task abort.
            serve(&addr_str, m2).await;
        });

        // Give the server a tick to bind.
        tokio::time::sleep(tokio::time::Duration::from_millis(30)).await;

        // Connect and send a health check.
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();

        let mut resp = vec![0u8; 512];
        let n = stream.read(&mut resp).await.unwrap();
        let text = String::from_utf8_lossy(&resp[..n]);
        assert!(text.contains("200 OK"), "expected 200 OK, got: {text}");
        assert!(text.contains("interlink-relayer"));
    }
}
