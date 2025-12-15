# Vitess Persistence Backend for Mara Pool

This directory contains all the necessary configuration files and documentation for deploying Vitess.io as a horizontally scalable persistence backend for the Mara Pool stratum mining pool.

## Overview

The Vitess persistence backend provides:
- **Horizontal sharding** for massive scale (1000+ shares/sec sustained)
- **Configurable batch inserts** for optimal throughput
- **Retry queue** with exponential backoff for transient failures
- **Automatic fallback** to file-based persistence on extended outages
- **Non-blocking semantics** to never impact mining operations

## Files in This Directory

- **schema.sql** - Database schema for the `share_events` table
- **vschema.json** - Vitess VSchema configuration for sharding
- **README.md** - This file

## Architecture

```
Mining Share Submission (Hot Path)
    ↓ (non-blocking try_send)
Async Channel (10,000 event buffer)
    ↓
Background Worker Task
    ↓ (batch accumulation)
Batch Buffer (100 events or 1s timeout)
    ↓
sqlx Connection Pool → vtgate → vttablet → MySQL
    ↓ (on failure)
Retry Queue (max 3 attempts, 10s backoff)
    ↓ (on max retries)
File Fallback (optional)
```

## Database Schema

The `share_events` table stores all mining share submissions with the following key features:

### Sharding Strategy
- **Sharding Key**: `user_identity` (via `shard_key` column)
- **Vindex Type**: Hash vindex for even distribution
- **Rationale**: Most queries filter by user (payouts, stats), and users naturally distribute load

### Indexes
- `idx_user_timestamp` - User-specific queries (most common)
- `idx_timestamp` - Global time-range queries (dashboards)
- `idx_block_found` - Quick block discovery lookups
- `idx_valid_user` - Payout calculations (valid shares by user)
- `idx_template` - Mining efficiency analysis

## Deployment Guide

### Prerequisites

- Vitess cluster deployed and running (vtgate accessible)
- MySQL 8.0+ compatible vttablet instances
- `vtctlclient` or access to vtctld for schema management
- Network connectivity from pool servers to vtgate

### Step 1: Create Keyspace

```bash
vtctlclient -server vtctld:15999 CreateKeyspace -keyspace_type NORMAL mining_pool
```

### Step 2: Initialize Shards

For a 2-shard deployment:

```bash
vtctlclient -server vtctld:15999 CreateShard mining_pool/-80
vtctlclient -server vtctld:15999 CreateShard mining_pool/80-
```

For more shards (e.g., 4 shards):

```bash
vtctlclient -server vtctld:15999 CreateShard mining_pool/-40
vtctlclient -server vtctld:15999 CreateShard mining_pool/40-80
vtctlclient -server vtctld:15999 CreateShard mining_pool/80-c0
vtctlclient -server vtctld:15999 CreateShard mining_pool/c0-
```

### Step 3: Apply Schema

```bash
vtctlclient -server vtctld:15999 ApplySchema -sql-file docs/vitess/schema.sql mining_pool
```

### Step 4: Apply VSchema

```bash
vtctlclient -server vtctld:15999 ApplyVSchema -vschema_file docs/vitess/vschema.json mining_pool
```

### Step 5: Initialize Tablets

Follow your Vitess deployment guide to initialize tablets for each shard. This typically involves:

1. Starting vttablet processes
2. Running `InitShardPrimary` to elect primary tablets
3. Configuring replication for replica tablets

### Step 6: Verify Setup

Test the connection from a pool server:

```bash
mysql -h vtgate -P 15306 -u pool_app -p -e "SELECT 1"
mysql -h vtgate -P 15306 -u pool_app -p -D mining_pool -e "SHOW TABLES"
```

## Pool Configuration

### Enable Vitess Feature

Build the pool with Vitess support:

```bash
cargo build --release --features vitess
```

### Configure Pool

Copy and customize the example configuration:

```bash
cp pool-apps/pool/config-examples/mainnet/pool-config-vitess-example.toml config.toml
```

Edit the `[persistence.vitess]` section:

```toml
[persistence]
backend = "vitess"
entities = ["shares"]

[persistence.vitess]
connection_string = "mysql://pool_app:your_password@vtgate.prod.internal:15306/mining_pool"
pool_size = 20
channel_size = 10000
batch_size = 100
batch_timeout_ms = 1000
retry_max_attempts = 3
retry_timeout_secs = 10
fallback_file_path = "/var/log/pool/share_events_fallback.log"
```

### Connection String Format

```
mysql://username:password@host:port/keyspace[?options]
```

**Examples:**

- **Local Development (direct MySQL):**
  ```
  mysql://root:password@localhost:3306/mining_pool_dev
  ```

- **Production (vtgate via load balancer):**
  ```
  mysql://pool_app:secure_pass@vtgate-lb.prod.internal:15306/mining_pool
  ```

- **SSL/TLS Enabled:**
  ```
  mysql://pool_app:pass@vtgate.prod:15306/mining_pool?ssl-mode=REQUIRED
  ```

## Configuration Tuning

### Batch Size

- **Default**: 100 events
- **Low traffic**: 50 events (lower latency)
- **High traffic**: 200 events (higher throughput)
- **Monitor**: Batch flush frequency (should be < 20/sec)

### Batch Timeout

- **Default**: 1000ms (1 second)
- **Low latency requirements**: 500ms
- **High throughput only**: 2000ms
- **Monitor**: P99 share submission latency

### Connection Pool Size

- **Formula**: `(num_workers * 2) + 1`
- **Default**: 10 connections
- **High concurrency**: 20-50 connections
- **Monitor**: Pool checkout duration and utilization

### Channel Size

- **Default**: 10,000 events (~5MB memory)
- **High burst traffic**: 20,000 events
- **Memory constrained**: 5,000 events
- **Monitor**: Channel full errors in logs

### Retry Configuration

- **retry_max_attempts**: 3 (default)
  - Increase for flaky networks
  - Decrease to fail over faster
- **retry_timeout_secs**: 10 (default)
  - Increase for scheduled maintenance windows
  - Decrease for faster failure detection

## Monitoring

### Key Metrics

| Metric | Source | Alert Threshold |
|--------|--------|----------------|
| Share insertion rate | Application logs | < 100/sec (underutilized) |
| Batch flush rate | Application logs | > 20/sec (inefficient) |
| Retry queue size | Application logs | > 5000 (connectivity issues) |
| Fallback activation | Application logs | Any occurrence (critical) |
| vtgate query P99 | Vitess metrics | > 100ms |
| Connection pool util | sqlx metrics | > 90% |

### Log Messages

**Normal Operation:**
```
INFO Vitess persistence backend initialized
DEBUG Flushed 100 events to Vitess
```

**Warnings:**
```
WARN Retry queue full, failing over to file backend
WARN Max retry attempts reached for event, using fallback
```

**Errors:**
```
ERROR Failed to insert batch: connection refused
ERROR Failed to send event to Vitess persistence: channel full
```

### Querying Metrics

**Check share insertion rate:**
```sql
SELECT COUNT(*) / 60 AS shares_per_sec
FROM share_events
WHERE inserted_at >= NOW() - INTERVAL 1 MINUTE;
```

**Check block discoveries:**
```sql
SELECT user_identity, COUNT(*) AS blocks_found
FROM share_events
WHERE is_block_found = TRUE
  AND inserted_at >= NOW() - INTERVAL 1 DAY
GROUP BY user_identity;
```

**Check error distribution:**
```sql
SELECT error_code, COUNT(*) AS count
FROM share_events
WHERE is_valid = FALSE
  AND inserted_at >= NOW() - INTERVAL 1 HOUR
GROUP BY error_code
ORDER BY count DESC;
```

## Operational Runbook

### Issue: High Retry Queue Size

**Symptoms:**
- Log messages: "Retry attempt X failed"
- Increasing retry queue depth

**Diagnosis:**
```bash
# Check vtgate connectivity
mysql -h vtgate -P 15306 -u pool_app -p -e "SELECT 1"

# Check vtgate logs
kubectl logs -l app=vtgate --tail=100

# Check vttablet health
vtctlclient -server vtctld:15999 ListAllTablets
```

**Mitigation:**
- Short-term: Increase `retry_timeout_secs` to reduce retry pressure
- Long-term: Scale vtgate replicas or vttablet resources

### Issue: Fallback File Growing Rapidly

**Symptoms:**
- Log message: "Flushed X events to file fallback"
- Disk space alerts for fallback file path

**Diagnosis:**
1. Check Vitess cluster health
2. Verify connection string validity
3. Check authentication credentials
4. Review vtgate/vttablet error logs

**Mitigation:**
1. Fix Vitess connectivity issue
2. Optionally: Replay fallback events (requires custom tooling)

### Issue: Share Submission Latency Spike

**Symptoms:**
- Miner complaints about slow responses
- High P99 latency metrics

**Diagnosis:**
```bash
# Check channel buffer saturation
grep "channel full" /var/log/pool/pool.log

# Check batch flush latency
grep "Flushed.*events" /var/log/pool/pool.log | tail -20
```

**Mitigation:**
- If channel full: Increase `channel_size`
- If slow flushes: Reduce `batch_size` or scale vtgate
- If database overload: Add vttablet replicas

## Performance Benchmarks

### Expected Throughput

| Scenario | Shares/sec | Batch Size | Pool Size | Memory |
|----------|-----------|------------|-----------|--------|
| Small pool | 10-100 | 50 | 5 | 5MB |
| Medium pool | 100-500 | 100 | 10 | 10MB |
| Large pool | 500-1000 | 200 | 20 | 15MB |
| Enterprise | 1000+ | 200 | 50 | 20MB |

### Latency Targets

- **P50**: < 10ms (non-blocking guarantee)
- **P95**: < 30ms
- **P99**: < 50ms

## Scaling Considerations

### Horizontal Scaling (Adding Shards)

Vitess supports online shard splitting via `Reshard` workflow:

```bash
# Split shard -80 into two new shards
vtctlclient -server vtctld:15999 Reshard \
  -source_shards=-80 \
  -target_shards=-40,40-80 \
  mining_pool.share_events
```

This process is online and non-blocking.

### Vertical Scaling

- **vtgate**: Add more replicas behind load balancer
- **vttablet**: Increase MySQL resources (CPU, RAM, IOPS)
- **Connection pool**: Increase `pool_size` proportionally

## Disaster Recovery

### Backup Strategy

Use Vitess backup tools to backup individual shards:

```bash
vtctlclient -server vtctld:15999 Backup <shard>
```

### Restore Process

```bash
vtctlclient -server vtctld:15999 RestoreFromBackup <tablet_alias>
```

### Fallback File Replay

If events were written to the fallback file during an outage, you can replay them manually:

1. Parse fallback file (Debug format)
2. Convert to SQL INSERT statements
3. Execute via vtgate

*Note: Automated replay tooling is planned for a future release.*

## Security Considerations

### Authentication

- Use strong passwords for database users
- Rotate credentials regularly
- Use SSL/TLS for production deployments

### Authorization

Grant minimal required privileges:

```sql
CREATE USER 'pool_app'@'%' IDENTIFIED BY 'secure_password';
GRANT INSERT, SELECT ON mining_pool.share_events TO 'pool_app'@'%';
```

### Network Security

- Restrict vtgate access via firewall rules
- Use private network for vtgate ↔ vttablet communication
- Enable Vitess authentication plugins if needed

## Troubleshooting

### Connection Refused

**Error**: `Failed to connect to Vitess: connection refused`

**Solutions**:
- Verify vtgate is running and listening on port 15306
- Check firewall rules allow pool server → vtgate connectivity
- Verify connection string hostname/IP is correct

### Authentication Failed

**Error**: `Connection test failed: access denied for user`

**Solutions**:
- Verify username and password in connection string
- Check user has been created on all vttablet instances
- Ensure user has INSERT and SELECT privileges

### Table Not Found

**Error**: `table 'share_events' not found`

**Solutions**:
- Verify schema was applied: `vtctlclient GetSchema <keyspace>/<shard>`
- Check keyspace name matches connection string
- Ensure table exists on all shards

### Slow Queries

**Error**: Batch inserts taking > 1 second

**Solutions**:
- Check vttablet CPU/memory utilization
- Verify MySQL slow query log on vttablet
- Consider adding vttablet replicas to distribute load
- Review index usage (should use PRIMARY key for INSERT)

## Further Reading

- [Vitess Documentation](https://vitess.io/docs/)
- [Vitess MySQL Compatibility](https://vitess.io/docs/reference/compatibility/mysql-compatibility/)
- [sqlx Documentation](https://docs.rs/sqlx/)
- [Stratum V2 Protocol](https://stratumprotocol.org/)

## Support

For issues specific to the Vitess integration:
1. Check application logs for detailed error messages
2. Review this README and troubleshooting section
3. Open an issue on the mara-pool GitHub repository

For Vitess-specific issues:
- [Vitess Slack](https://vitess.io/slack)
- [Vitess GitHub Issues](https://github.com/vitessio/vitess/issues)
