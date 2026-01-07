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
- **AlertManager** - http://localhost:9093
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

## Alerting

The monitoring stack includes Prometheus AlertManager for automated alert notifications via Slack.

### Configuring Slack Webhook

1. Create a Slack incoming webhook:
   - Go to https://api.slack.com/apps
   - Create a new app or use an existing one
   - Enable "Incoming Webhooks"
   - Create a webhook for your channel

2. Update the AlertManager config:
   ```bash
   # Edit alertmanager/alertmanager.yml
   # Replace YOUR/SLACK/WEBHOOK with your actual webhook path:
   slack_api_url: 'https://hooks.slack.com/services/YOUR/SLACK/WEBHOOK'
   ```

3. Configure channels (optional):
   - `#pool-alerts` - General alerts (warnings)
   - `#pool-alerts-critical` - Critical alerts only
   - `#pool-daily-recap` - Daily summary reports

4. Restart AlertManager:
   ```bash
   docker-compose -f monitoring/docker-compose.yml restart alertmanager
   ```

### Alert Rules

| Alert | Condition | Severity |
|-------|-----------|----------|
| `HashrateDropped` | Hashrate drops >50% in 5min | warning |
| `HashrateCriticalDrop` | Hashrate drops >80% in 5min | critical |
| `HighInvalidShareRate` | Invalid share rate >5% over 5min | warning |
| `InvalidShareSpike` | Invalid share rate >20% over 1min | critical |
| `NoBlocksFound` | No blocks found in 24 hours | warning |
| `MetricsEndpointDown` | Pool metrics unreachable | critical |
| `DailyRecap` | Daily summary at 00:00 UTC | info |

### Customizing Alert Rules

Edit `prometheus/alert_rules.yml` to modify thresholds:

```yaml
# Example: Change hashrate drop threshold to 40%
- alert: HashrateDropped
  expr: |
    (avg_over_time(stratum_pool_estimated_hashrate[10m] offset 5m) -
     avg_over_time(stratum_pool_estimated_hashrate[5m])) /
    avg_over_time(stratum_pool_estimated_hashrate[10m] offset 5m) > 0.4  # Changed from 0.5
  for: 2m
  labels:
    severity: warning
```

After editing, reload Prometheus:
```bash
curl -X POST http://localhost:9090/-/reload
```

### Testing Alerts

1. **View pending/firing alerts:**
   - Prometheus: http://localhost:9090/alerts
   - AlertManager: http://localhost:9093/#/alerts

2. **Manually trigger a test alert:**
   ```bash
   # Create a temporary alert via AlertManager API
   curl -X POST http://localhost:9093/api/v1/alerts \
     -H "Content-Type: application/json" \
     -d '[{
       "labels": {"alertname": "TestAlert", "severity": "warning"},
       "annotations": {"summary": "Test alert", "description": "This is a test alert"}
     }]'
   ```

3. **Silence alerts during maintenance:**
   - Visit http://localhost:9093/#/silences
   - Create a silence for specific alerts or time periods

### Daily Recap

The `DailyRecap` alert fires once per day at 00:00 UTC and sends a summary to Slack including:
- Total valid/invalid shares (24h)
- Blocks found (24h)
- Average and peak hashrate
- Invalid share rate percentage
- Active miners count

## Configuration Files

- `docker-compose.yml` - Service definitions
- `prometheus/prometheus.yml` - Prometheus scrape config
- `prometheus/alert_rules.yml` - Alert rule definitions
- `prometheus/recording_rules.yml` - Pre-computed metrics
- `alertmanager/alertmanager.yml` - AlertManager config (Slack)
- `alertmanager/templates/` - Slack message templates
- `loki/loki-config.yaml` - Loki storage config
- `promtail/promtail-config.yaml` - Log collection config
- `grafana/provisioning/` - Datasources and dashboards

All files in `/monitoring/` are git-ignored and kept locally only.
