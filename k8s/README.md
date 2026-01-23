# Kubernetes Deployment for Stratum V2 Mining

This directory contains Kubernetes manifests for deploying a complete Stratum V2 mining infrastructure.

## Components

| Component | Version | Description |
|-----------|---------|-------------|
| Bitcoin Node | v30.2 + sv2-tp v1.0.5 | Bitcoin Core with SV2 Template Provider |
| Pool | v0.2.0 | SV2 Mining Pool Server |
| Translator | v0.2.0 | SV1-to-SV2 Protocol Bridge |

## Architecture

```
                    ┌─────────────────────────────┐
                    │      Bitcoin Network        │
                    └─────────────┬───────────────┘
                                  │ P2P (8333)
                    ┌─────────────▼───────────────┐
                    │   Bitcoin Node (StatefulSet) │
                    │  ┌─────────┐  ┌──────────┐  │
                    │  │bitcoind │◄─┤  sv2-tp  │  │
                    │  └─────────┘  └────┬─────┘  │
                    └────────────────────┼────────┘
                                         │ SV2 TP (8442)
                    ┌────────────────────▼────────┐
                    │    Pool (Deployment x1)      │
                    │       stratumv2/pool_sv2    │
                    └────────────────┬────────────┘
                                     │ SV2 (34254)
                    ┌────────────────▼────────────┐
                    │ Translator (Deployment x2-10)│
                    │   stratumv2/translator_sv2  │
                    │         (HPA enabled)        │
                    └────────────────┬────────────┘
                                     │ SV1 (34255)
                           LoadBalancer Service
                                     │
                    ┌────────────────▼────────────┐
                    │        SV1 Miners           │
                    │    (external connections)    │
                    └─────────────────────────────┘
```

## Prerequisites

- Kubernetes cluster (1.24+)
- kubectl configured with cluster access
- Storage class for PersistentVolumeClaims
- (Optional) Prometheus Operator for ServiceMonitors
- Docker registry for custom Bitcoin node image

## Quick Start

### 1. Build the Bitcoin Node Image

```bash
cd k8s/bitcoin-node
docker build -t your-registry/bitcoin-sv2-node:v30.2 .
docker push your-registry/bitcoin-sv2-node:v30.2
```

### 2. Update Image Reference

Edit `k8s/base/kustomization.yaml` to point to your registry:

```yaml
images:
  - name: bitcoin-sv2-node
    newName: your-registry/bitcoin-sv2-node
    newTag: v30.2
```

### 3. Configure Secrets

Edit `k8s/base/secrets.yaml` with your values:

```yaml
stringData:
  AUTHORITY_PUBLIC_KEY: "your-public-key"
  AUTHORITY_SECRET_KEY: "your-secret-key"
```

> **Warning**: Never commit real secrets to version control!

### 4. Deploy

```bash
# Deploy with default configuration (signet)
kubectl apply -k k8s/

# Or deploy for a specific network
kubectl apply -k k8s/overlays/mainnet/
kubectl apply -k k8s/overlays/testnet4/
kubectl apply -k k8s/overlays/signet/
```

### 5. Monitor Deployment

```bash
# Watch pods come up
kubectl -n sv2-mining get pods -w

# Check Bitcoin node sync status
kubectl -n sv2-mining logs -f statefulset/bitcoin-node

# Check Pool logs
kubectl -n sv2-mining logs -f deployment/pool-sv2

# Check Translator logs
kubectl -n sv2-mining logs -f deployment/translator-sv2
```

### 6. Get Miner Connection Info

```bash
# Get LoadBalancer external IP
kubectl -n sv2-mining get svc translator-sv2

# Output example:
# NAME             TYPE           CLUSTER-IP      EXTERNAL-IP     PORT(S)
# translator-sv2   LoadBalancer   10.100.200.50   203.0.113.100   34255:30255/TCP
```

Miners connect to: `stratum+tcp://203.0.113.100:34255`

## Network Configuration

| Network | Command | Storage | Notes |
|---------|---------|---------|-------|
| Signet | `kubectl apply -k k8s/overlays/signet/` | 10Gi | Development |
| Testnet4 | `kubectl apply -k k8s/overlays/testnet4/` | 50Gi | Testing |
| Mainnet | `kubectl apply -k k8s/overlays/mainnet/` | 750Gi+ | Production |

## Configuration

### Bitcoin Node

Configuration is split between two ConfigMaps:

- `bitcoin-config`: Bitcoin Core settings (`bitcoin.conf`)
- `sv2tp-config`: Template Provider settings (`sv2-tp.conf`)

Key settings to customize:

| Setting | Location | Description |
|---------|----------|-------------|
| `chain` | bitcoin.conf | Network (main/testnet4/signet) |
| `prune` | bitcoin.conf | 0=full node, >550=pruned (MiB) |
| `sv2interval` | sv2-tp.conf | Template update interval (seconds) |

### Pool

Configuration in `pool/configmap.yaml`:

| Setting | Description |
|---------|-------------|
| `coinbase_reward_script` | Your Bitcoin address for block rewards |
| `pool_signature` | Text included in coinbase |
| `shares_per_minute` | Target share rate |

### Translator

Configuration in `translator/configmap.yaml`:

| Setting | Description |
|---------|-------------|
| `user_identity` | Username prefix for miners |
| `min_individual_miner_hashrate` | Minimum expected hashrate |
| `enable_vardiff` | Variable difficulty adjustment |

## Scaling

### Translator (Horizontal Scaling)

The Translator is configured with HPA (Horizontal Pod Autoscaler):

- Minimum replicas: 2
- Maximum replicas: 10
- Scale-up trigger: 70% CPU utilization

Manual scaling:

```bash
kubectl -n sv2-mining scale deployment translator-sv2 --replicas=5
```

### Pool (Not Scalable)

The Pool runs as a single replica. It maintains channel state and must be singleton.

### Bitcoin Node (Not Scalable)

The Bitcoin Node is a StatefulSet with 1 replica for blockchain data consistency.

## Monitoring

### Prometheus Integration

ServiceMonitors are included but commented out in `k8s/base/kustomization.yaml`. To enable:

1. Ensure Prometheus Operator is installed
2. Uncomment ServiceMonitor resources in `k8s/base/kustomization.yaml`
3. Re-apply: `kubectl apply -k k8s/`

### Health Endpoints

| Service | Endpoint | Description |
|---------|----------|-------------|
| Pool | `http://pool-sv2:8080/health` | Liveness check |
| Pool | `http://pool-sv2:8080/metrics` | Prometheus metrics |
| Translator | `http://translator-sv2:8080/health` | Liveness check |
| Translator | `http://translator-sv2:8080/metrics` | Prometheus metrics |

### Logs

```bash
# Bitcoin node (both bitcoind and sv2-tp)
kubectl -n sv2-mining logs statefulset/bitcoin-node

# Pool
kubectl -n sv2-mining logs deployment/pool-sv2

# Translator (all pods)
kubectl -n sv2-mining logs -l app.kubernetes.io/name=translator-sv2
```

## Troubleshooting

### Bitcoin Node Not Syncing

```bash
# Check bitcoind status
kubectl -n sv2-mining exec -it statefulset/bitcoin-node -- \
  bitcoin-cli -datadir=/data -conf=/config/bitcoin.conf getblockchaininfo

# Check P2P connectivity
kubectl -n sv2-mining get svc bitcoin-node-p2p
```

### Pool Can't Connect to Template Provider

```bash
# Verify sv2-tp is running
kubectl -n sv2-mining exec -it statefulset/bitcoin-node -- \
  supervisorctl status

# Check sv2-tp service
kubectl -n sv2-mining get svc bitcoin-node-tp
kubectl -n sv2-mining get endpoints bitcoin-node-tp
```

### Translator Can't Connect to Pool

```bash
# Verify Pool service
kubectl -n sv2-mining get svc pool-sv2
kubectl -n sv2-mining get endpoints pool-sv2

# Check Translator logs
kubectl -n sv2-mining logs -l app.kubernetes.io/name=translator-sv2 --tail=100
```

### Miners Can't Connect

```bash
# Check LoadBalancer status
kubectl -n sv2-mining get svc translator-sv2

# Verify Translator pods are ready
kubectl -n sv2-mining get pods -l app.kubernetes.io/name=translator-sv2

# Test port connectivity (from within cluster)
kubectl -n sv2-mining run debug --rm -it --image=busybox -- \
  nc -zv translator-sv2 34255
```

## Security Considerations

1. **Secrets**: Use external secrets management in production (Vault, AWS Secrets Manager, etc.)
2. **Network Policies**: Consider adding NetworkPolicy resources to restrict pod communication
3. **Authority Keys**: Generate new keys for production - never use the example keys
4. **RPC Access**: Bitcoin RPC is internal only (ClusterIP)
5. **Monitoring Port**: Consider using a separate internal service for monitoring endpoints

## File Structure

```
k8s/
├── CLAUDE.md                    # Design decisions and progress
├── README.md                    # This file
├── kustomization.yaml           # Root kustomization (points to base)
│
├── base/                        # Base configuration (signet default)
│   ├── kustomization.yaml      # Base kustomization
│   ├── namespace.yaml          # sv2-mining namespace
│   ├── secrets.yaml            # Authority keys and RPC credentials
│   │
│   ├── bitcoin-node/
│   │   ├── Dockerfile          # bitcoind + sv2-tp image
│   │   ├── supervisord.conf    # Process manager config
│   │   ├── entrypoint.sh       # Container startup
│   │   ├── configmap-bitcoin.yaml
│   │   ├── configmap-sv2tp.yaml
│   │   ├── statefulset.yaml
│   │   ├── service-rpc.yaml
│   │   ├── service-tp.yaml
│   │   ├── service-p2p.yaml
│   │   └── servicemonitor.yaml
│   │
│   ├── pool/
│   │   ├── configmap.yaml
│   │   ├── deployment.yaml
│   │   ├── service.yaml
│   │   └── servicemonitor.yaml
│   │
│   └── translator/
│       ├── configmap.yaml
│       ├── deployment.yaml
│       ├── service.yaml
│       ├── hpa.yaml
│       └── servicemonitor.yaml
│
└── overlays/
    ├── mainnet/                 # Mainnet configuration
    │   ├── kustomization.yaml
    │   └── patches/
    ├── testnet4/                # Testnet4 configuration
    │   ├── kustomization.yaml
    │   └── patches/
    └── signet/                  # Signet configuration
        ├── kustomization.yaml
        └── patches/
```

## References

- [Stratum V2 Protocol](https://stratumprotocol.org)
- [sv2-apps Repository](https://github.com/stratum-mining/sv2-apps)
- [sv2-tp Repository](https://github.com/stratum-mining/sv2-tp)
- [Bitcoin Core](https://bitcoincore.org)
