# Kubernetes Deployment for Stratum V2

## Project Overview

Kubernetes deployment for Stratum V2 mining infrastructure using:
- **sv2-apps v0.2.0** (Pool, Translator) - [Release Notes](https://github.com/stratum-mining/sv2-apps/releases/tag/v0.2.0)
- **sv2-tp v1.0.5** (Template Provider) - [Release Notes](https://github.com/stratum-mining/sv2-tp/releases/tag/v1.0.5)
- **Bitcoin Core v30.2**

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                           Kubernetes Cluster                                     │
│  Namespace: sv2-mining                                                           │
│                                                                                  │
│  ┌────────────────────────────────────────────────────────────────────────────┐ │
│  │  Bitcoin Node (StatefulSet, 1 replica)                                     │ │
│  │  ┌─────────────────────────────────────────────────────────────────────┐  │ │
│  │  │              SINGLE CONTAINER (supervisord)                          │  │ │
│  │  │  ┌───────────────────┐      ┌───────────────────┐                   │  │ │
│  │  │  │     bitcoind      │ IPC  │      sv2-tp       │                   │  │ │
│  │  │  │  (Bitcoin Core    │◄────►│  (Template        │                   │  │ │
│  │  │  │   v30.2)          │socket│   Provider v1.0.5)│                   │  │ │
│  │  │  └───────────────────┘      └───────────────────┘                   │  │ │
│  │  │                                                                      │  │ │
│  │  │  Ports: 8332 (RPC), 8333 (P2P), 8442 (sv2-tp)                       │  │ │
│  │  └─────────────────────────────────────────────────────────────────────┘  │ │
│  │                              │ PVC (blockchain data)                       │ │
│  └──────────────────────────────┼─────────────────────────────────────────────┘ │
│                                 │                                                │
│                                 │ sv2-tp Service (ClusterIP:8442)               │
│                                 ▼                                                │
│  ┌────────────────────────────────────────────────────────────────────────────┐ │
│  │  Pool (Deployment, 1 replica - NOT scalable)                               │ │
│  │  ┌─────────────────────────────────────────────────────────────────────┐  │ │
│  │  │  pool_sv2 v0.2.0                                                     │  │ │
│  │  │  template_provider: bitcoin-node-tp:8442 (network, decoupled)       │  │ │
│  │  │  Ports: 34254 (SV2), 8080 (HTTP monitoring)                         │  │ │
│  │  └─────────────────────────────────────────────────────────────────────┘  │ │
│  └────────────────────────────────────────────────────────────────────────────┘ │
│                                 │                                                │
│                                 │ pool-sv2 Service (ClusterIP:34254)            │
│                                 ▼                                                │
│  ┌────────────────────────────────────────────────────────────────────────────┐ │
│  │  Translator (Deployment + HPA, 2-10 replicas) - HORIZONTALLY SCALABLE     │ │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐                        │ │
│  │  │translator-0 │  │translator-1 │  │translator-N │                        │ │
│  │  │             │  │             │  │             │                        │ │
│  │  │ Ports:      │  │ Ports:      │  │ Ports:      │                        │ │
│  │  │ -34255(SV1) │  │ -34255(SV1) │  │ -34255(SV1) │                        │ │
│  │  │ -8080(HTTP) │  │ -8080(HTTP) │  │ -8080(HTTP) │                        │ │
│  │  └─────────────┘  └─────────────┘  └─────────────┘                        │ │
│  │                              ▲                                             │ │
│  │                              │ LoadBalancer Service                        │ │
│  └──────────────────────────────┼─────────────────────────────────────────────┘ │
│                                 │                                                │
└─────────────────────────────────┼────────────────────────────────────────────────┘
                                  │
                        ┌─────────┴─────────┐
                        │    SV1 Miners     │
                        │    (external)     │
                        └───────────────────┘
```

## Design Decisions

### 1. Bitcoin Node + sv2-tp in Single Container

**Decision**: Run bitcoind and sv2-tp together in a single container managed by supervisord.

**Rationale**:
- sv2-tp requires IPC socket access to bitcoind (`-ipcbind=unix`)
- Tightly coupled processes - if one fails, both should restart
- Simpler than sidecar pattern for IPC socket sharing
- No need for shared volumes between containers

**Implementation**: supervisord manages both processes with proper startup ordering (bitcoind first, then sv2-tp after socket is available).

### 2. Pool Uses Network Connection to sv2-tp (Not IPC)

**Decision**: Pool connects to sv2-tp via network (SV2 Template Distribution Protocol on port 8442) rather than direct IPC socket mount.

**Rationale**:
- Decouples Pool from Bitcoin Node pod
- Allows Pool to be restarted/updated independently
- Cleaner Kubernetes architecture (services vs shared volumes)
- Pool v0.2.0 supports both IPC and network template providers

### 3. Translator is Horizontally Scalable, Pool is Not

**Decision**: 
- Translator: Deployment with HPA (2-10 replicas)
- Pool: Deployment with fixed 1 replica

**Rationale**:
- **Translator**: Stateless protocol translation, can handle many SV1 miners in parallel
- **Pool**: Manages channel state, job distribution - needs to be singleton to maintain consistency
- HPA scales Translator based on CPU (70%) and memory (80%) utilization

### 4. Kustomize Overlays for Network Configuration

**Decision**: Use Kustomize with overlays for mainnet/testnet4/signet configurations.

**Rationale**:
- Single base configuration with network-specific patches
- Easy to switch between networks
- No templating engine required (native kubectl support)
- Clean separation of concerns

### 5. Secrets for Sensitive Configuration

**Decision**: Authority keys and RPC credentials stored in Kubernetes Secrets, referenced via environment variables.

**Rationale**:
- Don't commit secrets to git
- Easy integration with external secret managers (Vault, AWS SM, etc.)
- Init containers use envsubst to render config templates with secret values

### 6. ServiceMonitors for Prometheus Integration

**Decision**: Include ServiceMonitor CRDs for automatic Prometheus scraping.

**Rationale**:
- v0.2.0 adds HTTP monitoring APIs to Pool and Translator
- Native Prometheus Operator integration
- Automatic service discovery

### 7. Storage Configuration

**Decision**: Configurable PVC size via overlays.

| Network | Recommended Storage |
|---------|---------------------|
| Mainnet | 750Gi+ (full node) or 10Gi (pruned) |
| Testnet4 | 50Gi |
| Signet | 10Gi |

## Component Specifications

| Component | K8s Type | Image | Replicas | Ports |
|-----------|----------|-------|----------|-------|
| Bitcoin Node | StatefulSet | Custom Dockerfile | 1 | 8332 (RPC), 8333 (P2P), 8442 (sv2-tp) |
| Pool | Deployment | `stratumv2/pool_sv2:v0.2.0` | 1 | 34254 (SV2), 8080 (HTTP) |
| Translator | Deployment + HPA | `stratumv2/translator_sv2:v0.2.0` | 2-10 | 34255 (SV1), 8080 (HTTP) |

## Progress Tracker

### Files Created

#### Core Infrastructure (in `base/`)
- [x] `base/namespace.yaml` - sv2-mining namespace
- [x] `base/secrets.yaml` - Authority keys and RPC credentials template
- [x] `base/kustomization.yaml` - Base kustomization

#### Bitcoin Node (in `base/bitcoin-node/`)
- [x] `base/bitcoin-node/Dockerfile` - Multi-stage build for bitcoind v30.2 + sv2-tp v1.0.5
- [x] `base/bitcoin-node/supervisord.conf` - Process manager configuration
- [x] `base/bitcoin-node/entrypoint.sh` - Container startup script
- [x] `base/bitcoin-node/configmap-bitcoin.yaml` - bitcoin.conf
- [x] `base/bitcoin-node/configmap-sv2tp.yaml` - sv2-tp.conf
- [x] `base/bitcoin-node/statefulset.yaml` - StatefulSet definition
- [x] `base/bitcoin-node/service-rpc.yaml` - ClusterIP for RPC (internal)
- [x] `base/bitcoin-node/service-tp.yaml` - ClusterIP for sv2-tp (internal)
- [x] `base/bitcoin-node/service-p2p.yaml` - NodePort for P2P (external)
- [x] `base/bitcoin-node/servicemonitor.yaml` - Prometheus ServiceMonitor

#### Pool (in `base/pool/`)
- [x] `base/pool/configmap.yaml` - pool-config.toml with Sv2TemplateProvider
- [x] `base/pool/deployment.yaml` - Single replica deployment
- [x] `base/pool/service.yaml` - ClusterIP for SV2 + monitoring
- [x] `base/pool/servicemonitor.yaml` - Prometheus ServiceMonitor

#### Translator (in `base/translator/`)
- [x] `base/translator/configmap.yaml` - translator-config.toml
- [x] `base/translator/deployment.yaml` - HPA-ready deployment
- [x] `base/translator/service.yaml` - LoadBalancer for SV1 miners
- [x] `base/translator/hpa.yaml` - HorizontalPodAutoscaler
- [x] `base/translator/servicemonitor.yaml` - Prometheus ServiceMonitor

#### Kustomize Overlays
- [x] `kustomization.yaml` - Root kustomization (points to base)
- [x] `overlays/mainnet/kustomization.yaml`
- [x] `overlays/mainnet/patches/bitcoin-config.yaml`
- [x] `overlays/mainnet/patches/pool-config.yaml`
- [x] `overlays/mainnet/patches/storage.yaml`
- [x] `overlays/testnet4/kustomization.yaml`
- [x] `overlays/testnet4/patches/bitcoin-config.yaml`
- [x] `overlays/testnet4/patches/pool-config.yaml`
- [x] `overlays/signet/kustomization.yaml`
- [x] `overlays/signet/patches/bitcoin-config.yaml`
- [x] `overlays/signet/patches/pool-config.yaml`

#### Documentation
- [x] `README.md` - Deployment guide
- [x] `CLAUDE.md` - Design decisions and progress tracking

## Network Configuration Reference

| Setting | Mainnet | Testnet4 | Signet |
|---------|---------|----------|--------|
| `chain` | `main` | `testnet4` | `signet` |
| P2P Port | 8333 | 48333 | 38333 |
| RPC Port | 8332 | 48332 | 38332 |
| Coinbase Script | `bc1q...` | `tb1q...` | `tb1q...` |
| Default Storage | 750Gi | 50Gi | 10Gi |

## Security Considerations

1. **Secrets Management**: Use external secrets operator in production (Vault, AWS Secrets Manager)
2. **Network Policies**: Add NetworkPolicy resources to restrict pod communication
3. **RPC Access**: Bitcoin RPC exposed only via ClusterIP (internal)
4. **Authority Keys**: Generate new keys for production - defaults are for testing only
5. **Resource Limits**: Configured with sensible defaults, adjust based on load

## Quick Start

```bash
# Build Bitcoin node image
docker build -t your-registry/bitcoin-sv2-node:v30.2 -f k8s/base/bitcoin-node/Dockerfile k8s/base/bitcoin-node/
docker push your-registry/bitcoin-sv2-node:v30.2

# Deploy (signet by default)
kubectl apply -k k8s/

# Or deploy specific network
kubectl apply -k k8s/overlays/mainnet/
kubectl apply -k k8s/overlays/testnet4/

# Get LoadBalancer IP for miners
kubectl -n sv2-mining get svc translator-sv2
```

## Miner Connection

Once deployed, SV1 miners connect to:
```
stratum+tcp://<TRANSLATOR_LOADBALANCER_IP>:34255
```

## References

- [sv2-apps v0.2.0 Release](https://github.com/stratum-mining/sv2-apps/releases/tag/v0.2.0)
- [sv2-tp v1.0.5 Release](https://github.com/stratum-mining/sv2-tp/releases/tag/v1.0.5)
- [Stratum V2 Protocol](https://stratumprotocol.org)
- [Bitcoin Core v30.2](https://bitcoincore.org/en/releases/30.2/)
