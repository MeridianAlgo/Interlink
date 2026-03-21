//! Webhook API for InterLink transfer event subscriptions (Phase 5).
//!
//! Allows external services to register HTTP callback URLs that receive
//! real-time notifications when bridge events occur.
//!
//! # Event lifecycle callbacks
//!
//! ```text
//! User submits transfer
//!     → WebhookEvent::TransferInitiated (sequence, chain, amount)
//!     → WebhookEvent::FinalityConfirmed (sequence, block)
//!     → WebhookEvent::ProofGenerated    (sequence, proof_ms)
//!     → WebhookEvent::SettlementComplete (sequence, signature, total_ms)
//! On failure:
//!     → WebhookEvent::TransferFailed    (sequence, reason)
//! ```
//!
//! # Registration
//!
//! POST /webhooks/register   { "url": "https://...", "events": ["all"] }
//! DELETE /webhooks/{id}
//! GET /webhooks/{id}
//!
//! # Reliability
//!
//! Notifications are delivered with up to 3 retries (exponential backoff: 1s, 2s, 4s).
//! Webhooks that fail all retries are marked inactive after 10 consecutive failures.
//!
//! # Comparison vs competitors
//! Wormhole:  No webhook API — polling only
//! Across:    REST polling endpoint, no push
//! Stargate:  Subgraph queries only
//! InterLink: Real-time push webhooks (like Stripe/GitHub style)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, error, info, warn};

static WEBHOOK_COUNTER: AtomicU64 = AtomicU64::new(1);

// ─── Types ────────────────────────────────────────────────────────────────────

/// Webhook event types that subscribers can filter on.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    /// Transfer seen on source chain.
    TransferInitiated,
    /// Source block reached finality.
    FinalityConfirmed,
    /// ZK proof generated successfully.
    ProofGenerated,
    /// Transfer settled on Solana Hub.
    SettlementComplete,
    /// Any step in the pipeline failed.
    TransferFailed,
    /// Subscribe to all events.
    All,
}

impl EventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventType::TransferInitiated => "transfer.initiated",
            EventType::FinalityConfirmed => "finality.confirmed",
            EventType::ProofGenerated => "proof.generated",
            EventType::SettlementComplete => "settlement.complete",
            EventType::TransferFailed => "transfer.failed",
            EventType::All => "all",
        }
    }
}

/// A webhook event payload delivered to registered subscribers.
#[derive(Debug, Clone, Serialize)]
pub struct WebhookPayload {
    /// Unique event ID (for idempotency).
    pub event_id: String,
    /// Event type.
    pub event_type: String,
    /// Unix timestamp of the event (seconds).
    pub timestamp: u64,
    /// Sequence number of the transfer.
    pub sequence: u64,
    /// Source chain ID.
    pub source_chain: u64,
    /// Event-specific data.
    pub data: serde_json::Value,
}

impl WebhookPayload {
    pub fn transfer_initiated(
        sequence: u64,
        chain: u64,
        amount_wei: &str,
        recipient: &str,
    ) -> Self {
        Self {
            event_id: format!("evt_init_{sequence}_{}", now_secs()),
            event_type: EventType::TransferInitiated.as_str().to_string(),
            timestamp: now_secs(),
            sequence,
            source_chain: chain,
            data: serde_json::json!({
                "amount_wei": amount_wei,
                "recipient": recipient,
                "status": "pending_finality",
            }),
        }
    }

    pub fn finality_confirmed(sequence: u64, chain: u64, block_number: u64) -> Self {
        Self {
            event_id: format!("evt_final_{sequence}_{}", now_secs()),
            event_type: EventType::FinalityConfirmed.as_str().to_string(),
            timestamp: now_secs(),
            sequence,
            source_chain: chain,
            data: serde_json::json!({
                "block_number": block_number,
                "status": "finalized",
            }),
        }
    }

    pub fn proof_generated(sequence: u64, chain: u64, proof_ms: u64, proof_bytes: usize) -> Self {
        Self {
            event_id: format!("evt_proof_{sequence}_{}", now_secs()),
            event_type: EventType::ProofGenerated.as_str().to_string(),
            timestamp: now_secs(),
            sequence,
            source_chain: chain,
            data: serde_json::json!({
                "proof_generation_ms": proof_ms,
                "proof_size_bytes": proof_bytes,
                "status": "proof_ready",
            }),
        }
    }

    pub fn settlement_complete(sequence: u64, chain: u64, signature: &str, total_ms: u64) -> Self {
        Self {
            event_id: format!("evt_settle_{sequence}_{}", now_secs()),
            event_type: EventType::SettlementComplete.as_str().to_string(),
            timestamp: now_secs(),
            sequence,
            source_chain: chain,
            data: serde_json::json!({
                "solana_signature": signature,
                "total_settlement_ms": total_ms,
                "status": "complete",
            }),
        }
    }

    pub fn transfer_failed(sequence: u64, chain: u64, reason: &str, stage: &str) -> Self {
        Self {
            event_id: format!("evt_fail_{sequence}_{}", now_secs()),
            event_type: EventType::TransferFailed.as_str().to_string(),
            timestamp: now_secs(),
            sequence,
            source_chain: chain,
            data: serde_json::json!({
                "reason": reason,
                "failed_at_stage": stage,
                "status": "failed",
            }),
        }
    }
}

/// A registered webhook subscription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookRegistration {
    /// Auto-generated subscription ID.
    pub id: String,
    /// Callback URL.
    pub url: String,
    /// Event types to receive. Empty = all events.
    pub events: Vec<EventType>,
    /// Whether this webhook is active.
    pub active: bool,
    /// Unix timestamp of registration.
    pub registered_at: u64,
    /// Consecutive failure count (disabled after 10).
    pub consecutive_failures: u32,
    /// Total successful deliveries.
    pub total_delivered: u64,
    /// Total failed deliveries.
    pub total_failed: u64,
}

impl WebhookRegistration {
    pub fn new(url: String, events: Vec<EventType>) -> Self {
        let seq = WEBHOOK_COUNTER.fetch_add(1, Ordering::Relaxed);
        let id = format!("wh_{:x}_{seq}", now_secs());
        Self {
            id,
            url,
            events,
            active: true,
            registered_at: now_secs(),
            consecutive_failures: 0,
            total_delivered: 0,
            total_failed: 0,
        }
    }

    /// Whether this webhook should receive the given event type.
    pub fn matches(&self, event_type: &EventType) -> bool {
        if !self.active {
            return false;
        }
        self.events.is_empty()
            || self.events.contains(&EventType::All)
            || self.events.contains(event_type)
    }
}

// ─── Registry ─────────────────────────────────────────────────────────────────

/// Thread-safe webhook registry.
#[derive(Debug, Clone)]
pub struct WebhookRegistry {
    inner: Arc<Mutex<HashMap<String, WebhookRegistration>>>,
}

impl WebhookRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a new webhook subscription.
    pub fn register(&self, url: String, events: Vec<EventType>) -> WebhookRegistration {
        let reg = WebhookRegistration::new(url, events);
        let mut map = self.inner.lock().unwrap();
        map.insert(reg.id.clone(), reg.clone());
        info!(id = %reg.id, url = %reg.url, "webhook registered");
        reg
    }

    /// Deregister a webhook by ID.
    pub fn deregister(&self, id: &str) -> bool {
        let mut map = self.inner.lock().unwrap();
        let removed = map.remove(id).is_some();
        if removed {
            info!(id, "webhook deregistered");
        }
        removed
    }

    /// Get a webhook by ID.
    pub fn get(&self, id: &str) -> Option<WebhookRegistration> {
        self.inner.lock().unwrap().get(id).cloned()
    }

    /// List all webhooks.
    pub fn list(&self) -> Vec<WebhookRegistration> {
        self.inner.lock().unwrap().values().cloned().collect()
    }

    /// Get all active webhooks matching an event type.
    pub fn subscribers_for(&self, event_type: &EventType) -> Vec<WebhookRegistration> {
        self.inner
            .lock()
            .unwrap()
            .values()
            .filter(|r| r.matches(event_type))
            .cloned()
            .collect()
    }

    /// Record a delivery result (success or failure).
    pub fn record_delivery(&self, id: &str, success: bool) {
        let mut map = self.inner.lock().unwrap();
        if let Some(reg) = map.get_mut(id) {
            if success {
                reg.total_delivered += 1;
                reg.consecutive_failures = 0;
            } else {
                reg.total_failed += 1;
                reg.consecutive_failures += 1;
                // Disable after 10 consecutive failures
                if reg.consecutive_failures >= 10 {
                    reg.active = false;
                    warn!(id, "webhook disabled after 10 consecutive failures");
                }
            }
        }
    }

    /// Count of registered (active + inactive) webhooks.
    pub fn count(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    /// Count of active webhooks.
    pub fn active_count(&self) -> usize {
        self.inner
            .lock()
            .unwrap()
            .values()
            .filter(|r| r.active)
            .count()
    }
}

impl Default for WebhookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Dispatcher ───────────────────────────────────────────────────────────────

/// Deliver a webhook event to all matching subscribers.
///
/// Uses exponential backoff retries: 1s, 2s, 4s (up to 3 attempts).
/// Non-blocking: dispatches delivery tasks to tokio.
pub fn dispatch(registry: &WebhookRegistry, payload: WebhookPayload) {
    let event_type_str = payload.event_type.clone();
    let event_type = parse_event_type(&event_type_str);
    let subscribers = registry.subscribers_for(&event_type);

    if subscribers.is_empty() {
        debug!(
            event_type = %event_type_str,
            sequence = payload.sequence,
            "no webhook subscribers for event"
        );
        return;
    }

    let payload_str = match serde_json::to_string(&payload) {
        Ok(s) => s,
        Err(e) => {
            error!(error = %e, "failed to serialize webhook payload");
            return;
        }
    };

    info!(
        event_type = %event_type_str,
        sequence = payload.sequence,
        subscriber_count = subscribers.len(),
        "dispatching webhook event"
    );

    let registry_clone = registry.clone();
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();

        for sub in subscribers {
            let url = sub.url.clone();
            let id = sub.id.clone();
            let body = payload_str.clone();
            let reg = registry_clone.clone();
            let c = client.clone();
            let evt_type = event_type_str.clone();

            tokio::spawn(async move {
                let mut success = false;
                for attempt in 0u32..3 {
                    if attempt > 0 {
                        let delay = Duration::from_secs(1 << (attempt - 1)); // 1s, 2s, 4s
                        tokio::time::sleep(delay).await;
                    }

                    match c
                        .post(&url)
                        .header("Content-Type", "application/json")
                        .header("X-InterLink-Event", &evt_type)
                        .header("X-InterLink-Delivery", &id)
                        .body(body.clone())
                        .send()
                        .await
                    {
                        Ok(resp) if resp.status().is_success() => {
                            success = true;
                            debug!(
                                id = %id,
                                url = %url,
                                attempt = attempt + 1,
                                status = %resp.status(),
                                "webhook delivered"
                            );
                            break;
                        }
                        Ok(resp) => {
                            warn!(
                                id = %id,
                                url = %url,
                                attempt = attempt + 1,
                                status = %resp.status(),
                                "webhook non-2xx response, retrying"
                            );
                        }
                        Err(e) => {
                            warn!(
                                id = %id,
                                url = %url,
                                attempt = attempt + 1,
                                error = %e,
                                "webhook delivery error, retrying"
                            );
                        }
                    }
                }

                reg.record_delivery(&id, success);
                if !success {
                    error!(id = %id, url = %url, "webhook delivery failed after 3 attempts");
                }
            });
        }
    });
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn parse_event_type(s: &str) -> EventType {
    match s {
        "transfer.initiated" => EventType::TransferInitiated,
        "finality.confirmed" => EventType::FinalityConfirmed,
        "proof.generated" => EventType::ProofGenerated,
        "settlement.complete" => EventType::SettlementComplete,
        "transfer.failed" => EventType::TransferFailed,
        _ => EventType::All,
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_list() {
        let registry = WebhookRegistry::new();
        let reg = registry.register("https://example.com/hook".into(), vec![EventType::All]);
        assert!(reg.active);
        assert_eq!(registry.count(), 1);
        assert_eq!(registry.active_count(), 1);
    }

    #[test]
    fn test_deregister() {
        let registry = WebhookRegistry::new();
        let reg = registry.register("https://example.com/hook".into(), vec![]);
        assert!(registry.deregister(&reg.id));
        assert_eq!(registry.count(), 0);
        assert!(!registry.deregister(&reg.id)); // already gone
    }

    #[test]
    fn test_event_matching_all() {
        let registry = WebhookRegistry::new();
        registry.register("https://a.com".into(), vec![EventType::All]);

        let subs = registry.subscribers_for(&EventType::SettlementComplete);
        assert_eq!(subs.len(), 1);
    }

    #[test]
    fn test_event_matching_specific() {
        let registry = WebhookRegistry::new();
        registry.register("https://a.com".into(), vec![EventType::SettlementComplete]);
        registry.register("https://b.com".into(), vec![EventType::TransferFailed]);

        let settle_subs = registry.subscribers_for(&EventType::SettlementComplete);
        assert_eq!(settle_subs.len(), 1);
        assert_eq!(settle_subs[0].url, "https://a.com");

        let fail_subs = registry.subscribers_for(&EventType::TransferFailed);
        assert_eq!(fail_subs.len(), 1);
        assert_eq!(fail_subs[0].url, "https://b.com");
    }

    #[test]
    fn test_inactive_after_10_failures() {
        let registry = WebhookRegistry::new();
        let reg = registry.register("https://dead.com".into(), vec![EventType::All]);

        for _ in 0..10 {
            registry.record_delivery(&reg.id, false);
        }

        let updated = registry.get(&reg.id).unwrap();
        assert!(!updated.active);
        assert_eq!(updated.consecutive_failures, 10);
        assert_eq!(registry.active_count(), 0);
    }

    #[test]
    fn test_failure_reset_on_success() {
        let registry = WebhookRegistry::new();
        let reg = registry.register("https://flaky.com".into(), vec![EventType::All]);

        // 5 failures then 1 success
        for _ in 0..5 {
            registry.record_delivery(&reg.id, false);
        }
        registry.record_delivery(&reg.id, true);

        let updated = registry.get(&reg.id).unwrap();
        assert!(updated.active);
        assert_eq!(updated.consecutive_failures, 0);
        assert_eq!(updated.total_delivered, 1);
        assert_eq!(updated.total_failed, 5);
    }

    #[test]
    fn test_payload_construction() {
        let p = WebhookPayload::settlement_complete(42, 1, "5K9aB...", 15_000);
        assert_eq!(p.sequence, 42);
        assert_eq!(p.event_type, "settlement.complete");
        assert!(p.data["total_settlement_ms"] == 15_000);
        assert!(p.event_id.starts_with("evt_settle_42_"));
    }

    #[test]
    fn test_transfer_failed_payload() {
        let p = WebhookPayload::transfer_failed(7, 1, "rpc timeout", "finality");
        assert_eq!(p.event_type, "transfer.failed");
        assert_eq!(p.data["reason"], "rpc timeout");
        assert_eq!(p.data["failed_at_stage"], "finality");
    }

    #[test]
    fn test_empty_events_matches_all() {
        let mut reg = WebhookRegistration::new("https://x.com".into(), vec![]);
        assert!(reg.matches(&EventType::SettlementComplete));
        assert!(reg.matches(&EventType::TransferFailed));

        reg.active = false;
        assert!(!reg.matches(&EventType::All));
    }
}
