# Bitcoin Core (multiprocess) + SV2 Template Provider
# Multi-stage build for Bitcoin Core v30.2 with IPC support and sv2-tp v1.0.5
#
# Build:
#   docker build -t bitcoin-sv2-node:v30.2 .
#
# Run:
#   docker run -v /data/bitcoin:/data -p 8333:8333 -p 8442:8442 bitcoin-sv2-node:v30.2

ARG BITCOIN_VERSION=30.2
ARG SV2TP_VERSION=1.0.5

# =============================================================================
# Stage 1: Build Bitcoin Core with multiprocess/IPC support
# =============================================================================
FROM debian:bookworm AS bitcoin-builder

ARG BITCOIN_VERSION

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    autoconf \
    automake \
    build-essential \
    ca-certificates \
    cmake \
    git \
    libtool \
    pkg-config \
    wget \
    # Bitcoin Core dependencies
    libboost-dev \
    libevent-dev \
    libsqlite3-dev \
    # IPC/multiprocess dependencies
    capnproto \
    libcapnp-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Download and extract Bitcoin Core source
RUN wget -q "https://bitcoincore.org/bin/bitcoin-core-${BITCOIN_VERSION}/bitcoin-${BITCOIN_VERSION}.tar.gz" \
    && tar -xzf "bitcoin-${BITCOIN_VERSION}.tar.gz" \
    && rm "bitcoin-${BITCOIN_VERSION}.tar.gz"

WORKDIR /build/bitcoin-${BITCOIN_VERSION}

# Build Bitcoin Core with multiprocess/IPC support
# -DENABLE_IPC=ON enables the multiprocess binaries (bitcoin-node, etc.)
RUN cmake -B build \
    -DBUILD_BENCH=OFF \
    -DBUILD_TESTS=OFF \
    -DBUILD_GUI=OFF \
    -DBUILD_WALLET=OFF \
    -DENABLE_IPC=ON \
    -DCMAKE_BUILD_TYPE=Release \
    && cmake --build build -j$(nproc) \
    && cmake --install build --prefix=/opt/bitcoin

# =============================================================================
# Stage 2: Download sv2-tp
# =============================================================================
FROM debian:bookworm-slim AS sv2tp-downloader

ARG SV2TP_VERSION
ARG TARGETARCH

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    wget \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /tmp

# Download sv2-tp from GitHub releases
RUN set -ex \
    && ARCH_SUFFIX=$(case ${TARGETARCH} in \
        amd64) echo "x86_64-linux-gnu" ;; \
        arm64) echo "aarch64-linux-gnu" ;; \
        *) echo "Unsupported architecture: ${TARGETARCH}" && exit 1 ;; \
    esac) \
    && wget -q "https://github.com/stratum-mining/sv2-tp/releases/download/v${SV2TP_VERSION}/sv2-tp-${SV2TP_VERSION}-${ARCH_SUFFIX}.tar.gz" \
    && tar -xzf "sv2-tp-${SV2TP_VERSION}-${ARCH_SUFFIX}.tar.gz" \
    && mv "sv2-tp-${SV2TP_VERSION}/bin/sv2-tp" /usr/local/bin/ \
    && rm -rf /tmp/*

# =============================================================================
# Stage 3: Runtime image
# =============================================================================
FROM debian:bookworm-slim

LABEL org.opencontainers.image.title="Bitcoin Core (multiprocess) + SV2 Template Provider"
LABEL org.opencontainers.image.description="Bitcoin Core v30.2 with IPC support and Stratum V2 Template Provider v1.0.5"
LABEL org.opencontainers.image.source="https://github.com/stratum-mining/sv2-tp"
LABEL org.opencontainers.image.version="30.2-sv2tp-1.0.5"

# Install runtime dependencies and supervisord
RUN apt-get update && apt-get install -y --no-install-recommends \
    supervisor \
    ca-certificates \
    # Runtime libraries for Bitcoin Core multiprocess
    libevent-2.1-7 \
    libevent-core-2.1-7 \
    libevent-extra-2.1-7 \
    libevent-pthreads-2.1-7 \
    libsqlite3-0 \
    libcapnp-0.9.2 \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /data /config /var/log/supervisor

# Copy binaries from builder stages
# bitcoin-node is the multiprocess-enabled node binary (replaces bitcoind for IPC)
# Note: bitcoin-node is installed to libexec/, not bin/
COPY --from=bitcoin-builder /opt/bitcoin/libexec/bitcoin-node /usr/local/bin/
COPY --from=bitcoin-builder /opt/bitcoin/bin/bitcoin-cli /usr/local/bin/
# Also copy the bitcoin wrapper script for convenience
COPY --from=bitcoin-builder /opt/bitcoin/bin/bitcoin /usr/local/bin/
COPY --from=sv2tp-downloader /usr/local/bin/sv2-tp /usr/local/bin/

# Copy configuration files
COPY supervisord.conf /etc/supervisor/conf.d/supervisord.conf
COPY entrypoint.sh /entrypoint.sh

RUN chmod +x /entrypoint.sh

# Bitcoin Core ports
# 8332 - RPC (mainnet)
# 8333 - P2P (mainnet)
# 18332/18333 - RPC/P2P (testnet - legacy)
# 38332/38333 - RPC/P2P (signet)
# 48332/48333 - RPC/P2P (testnet4)
EXPOSE 8332 8333 18332 18333 38332 38333 48332 48333

# SV2 Template Provider port
EXPOSE 8442

# Data volume for blockchain
VOLUME ["/data"]

# Config volume
VOLUME ["/config"]

ENTRYPOINT ["/entrypoint.sh"]
CMD ["supervisord", "-c", "/etc/supervisor/conf.d/supervisord.conf"]
