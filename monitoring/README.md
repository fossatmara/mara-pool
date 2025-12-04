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

The pool must be started with the metrics backend enabled. Update your pool config:

```toml
[persistence]
entities = ["shares"]
backend = "metrics"

[persistence.metrics]
resource_path = "/metrics"
port = 9091
```

Then start the pool with the `metrics` feature:

```bash
cd pool-apps/pool
cargo run --features metrics -- -c <your-config.toml>
```

## Verifying Metrics

1. **Check pool metrics endpoint:**
   ```bash
   curl http://localhost:9091/metrics
   ```
   You should see metrics like:
   - `stratum_pool_shares_valid_total`
   - `stratum_pool_shares_invalid_total`
   - `stratum_pool_blocks_found_total`

2. **Check Prometheus is scraping:**
   - Go to http://localhost:9090/targets
   - Look for `sv2-pool` job - should show "UP"

3. **View in Grafana:**
   - Go to http://localhost:3000
   - Login: admin/admin
   - Navigate to: Dashboards → sv2 → SV2 Pool Overview

## Collecting Pool Logs

To send pool logs to Loki via Promtail, start your pool container with the label:

```bash
docker run --label logging=promtail your-pool-image
```

Or if running the pool directly (not in Docker), logs will appear in your terminal and can be viewed in Grafana's Explore tab by querying Loki directly.

## Metrics Available

### Share Metrics
- `stratum_pool_shares_valid_total{user_identity, template_id}` - Valid shares by user and template
- `stratum_pool_shares_invalid_total{user_identity, error_code}` - Invalid shares by user and error type

### Block Metrics
- `stratum_pool_blocks_found_total{user_identity}` - Blocks found by user

## Troubleshooting

### No metrics in Grafana
1. Verify pool is running with `--features metrics`
2. Check pool metrics: `curl http://localhost:9091/metrics`
3. Check Prometheus targets: http://localhost:9090/targets
4. Verify pool config has `[persistence.metrics]` section

### No logs in Grafana
1. Promtail only collects Docker container logs with label `logging=promtail`
2. For local pool development, logs appear in terminal (not collected by Promtail)
3. To view pool logs in Grafana, run pool in Docker with the logging label

### Prometheus shows "DOWN"
- If using Docker Desktop on Mac, `host.docker.internal` should work
- If on Linux, you may need to use your host IP instead
- Check pool is actually running on port 9091

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
