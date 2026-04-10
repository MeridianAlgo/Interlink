# Operational Runbook

## Monitoring Relayer Health
All telemetry is exposed over the Prometheus/Grafana endpoint initialized inside `metrics.rs`.
Track queue depth, `settlement_ms_max` and `proof_gen_ms_max`. If proof generation consistently breaches 100ms per batch tier, evaluate CPU core allocations on the designated prover pipeline.

## Responding to Security Incidents
Triggers in `circuitbreaker.rs` denote emergent incidents. If the `SETTLEMENT_ALERT_MS` or payload anomaly bounds fire:
1. Verify the on-chain gateway limits.
2. Immediately trigger the 3-of-n multi-sig emergency freeze.
3. Validate `audit_trail.rs` SHA-256 event chains for corrupted event insertions.

## Upgrading Contracts Without Downtime
Use the deployed `wrapped.rs` proxy resolution mappings. Smart contract vaults can migrate functionality asynchronously while `retry.rs` handles the exponential backoff buffering queues, assuring transactions resolve automatically once the new v2 endpoints go live.
