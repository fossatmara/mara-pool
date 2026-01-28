# SV2 Pool Monitoring Stack

Complete observability stack for Stratum V2 pool with Prometheus metrics and Loki logs.

## Quick Start

```bash
# Start the monitoring stack
docker-compose -f monitoring/docker-compose.yml up -d

# View logs
docker-compose -f monitoring/docker-compose.yml logs -f

# Stop the stack
docker-compose -f monitoring/docker-compose.yml down
```

## Services

- **Grafana** - http://localhost:3000 (admin/admin)
- **Prometheus** - http://localhost:9090
- **Loki** - http://localhost:3100
- **Promtail** - Collects Docker logs

## Pool Configuration

The pool and translator must expose their monitoring endpoints on `0.0.0.0` so Docker can
scrape them via `host.docker.internal`.

Update your pool config:

```toml
monitoring_address = "0.0.0.0:9091"
monitoring_cache_refresh_secs = 60
```

And ensure the translator config has a LAN-accessible monitoring address:

```toml
monitoring_address = "0.0.0.0:9092"
monitoring_cache_refresh_secs = 60
```

Then start the pool and translator:

```bash
cd pool-apps/pool
cargo run -- -c <your-config.toml>
```

```bash
cd miner-apps/translator
cargo run -- -c <your-translator-config.toml>
```

## Verifying Metrics

1. **Check pool metrics endpoint:**
   ```bash
   curl http://localhost:9091/metrics
   ```
   You should see metrics like:
   - `stratum_connections{direction="client"}` - Connection count
   - `stratum_channels{direction,channel_type}` - Channel counts
   - `stratum_hashrate{direction}` - Hashrate
   - `stratum_shares_total{direction,channel_type}` - Share counter
   - `stratum_messages_total{direction,msg_type}` - Message counter
   - `stratum_bytes_total{direction}` - Bytes transferred

2. **Check translator metrics endpoint:**
   ```bash
   curl http://localhost:9092/metrics
   ```
   Additional SV1 metrics:
   - `sv1_connections` - SV1 miner count
   - `sv1_hashrate` - SV1 miner hashrate
   - `sv1_shares_total` - SV1 share counter

3. **Check Prometheus is scraping:**
   - Go to http://localhost:9090/targets
   - Look for `mara-pool-pool` and `mara-pool-tproxy` jobs - should show "UP"

4. **View in Grafana:**
   - Go to http://localhost:3000
   - Login: admin/admin
   - Navigate to: Dashboards → Stratum Observability

## Collecting Pool Logs

To send pool logs to Loki via Promtail, start your pool container with the label:

```bash
docker run --label logging=promtail your-pool-image
```

Or if running the pool directly (not in Docker), logs will appear in your terminal and can be viewed in Grafana's Explore tab by querying Loki directly.

## Metrics Available

### Snapshot Metrics (Gauges)
- `stratum_connections{direction}` - Connection counts by direction
- `stratum_channels{direction,channel_type}` - Channel counts by direction and type
- `stratum_hashrate{direction}` - Hashrate by direction

### Event Metrics (Counters)
- `stratum_shares_total{direction,channel_type}` - Shares by direction and channel type
- `stratum_shares_rejected_total{reason}` - Rejected shares by reason
- `stratum_messages_total{direction,msg_type}` - Protocol messages by direction and type
- `stratum_bytes_total{direction}` - Bytes transferred by direction

### SV1 Metrics (Translator only)
- `sv1_connections` - SV1 miner connection count
- `sv1_hashrate` - SV1 miner hashrate
- `sv1_shares_total` - SV1 shares accepted

### Pool-specific Metrics
- `pool_templates_received_total` - Templates received from template provider
- `pool_blocks_found_total` - Blocks found by the pool

## Troubleshooting

### No metrics in Grafana
1. Check pool metrics: `curl http://localhost:9091/metrics`
2. Check translator metrics: `curl http://localhost:9092/metrics`
3. Check Prometheus targets: http://localhost:9090/targets

### No logs in Grafana
1. Promtail only collects Docker container logs with label `logging=promtail`
2. For local pool development, logs appear in terminal (not collected by Promtail)
3. To view pool logs in Grafana, run pool in Docker with the logging label

### Prometheus shows "DOWN"
- If using Docker Desktop on Mac, `host.docker.internal` should work
- If on Linux, you may need to use your host IP instead
- Check pool is actually running on port 9090 and translator on port 9092

## Dashboard Panels

The SV2 Pool Overview dashboard includes:
1. **Valid Share Rate** - Shares/sec by user and template
2. **Invalid Share Rate** - Rejected shares by user and error
3. **Total Valid Shares** - Cumulative count
4. **Total Invalid Shares** - Cumulative count  
5. **Total Blocks Found** - Cumulative count
6. **Share Acceptance Rate** - Percentage of valid shares
7. **Blocks Found per Hour** - Block discovery rate by user

## Configuration Files

- `docker-compose.yml` - Service definitions
- `prometheus/prometheus.yml` - Prometheus scrape config
- `loki/loki-config.yaml` - Loki storage config
- `promtail/promtail-config.yaml` - Log collection config
- `grafana/provisioning/` - Datasources and dashboards

All files in `/monitoring/` are git-ignored and kept locally only.
