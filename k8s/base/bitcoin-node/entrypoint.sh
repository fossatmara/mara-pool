#!/bin/bash
set -e

# Bitcoin Core + SV2 Template Provider Entrypoint
# This script prepares the environment before starting supervisord

echo "=== Bitcoin SV2 Node Starting ==="
echo "Bitcoin Core + SV2 Template Provider"
echo "===================================="

# Create necessary directories
mkdir -p /data/.bitcoin
mkdir -p /var/log/supervisor

# Ensure proper permissions on data directory
# (useful when running with mounted volumes)
if [ -d "/data" ]; then
    echo "Data directory: /data"
    echo "Contents:"
    ls -la /data/ 2>/dev/null || echo "  (empty)"
fi

# Check for config files
if [ ! -f "/config/bitcoin.conf" ]; then
    echo "WARNING: /config/bitcoin.conf not found!"
    echo "Please mount a ConfigMap with bitcoin.conf"
fi

if [ ! -f "/config/sv2-tp.conf" ]; then
    echo "WARNING: /config/sv2-tp.conf not found!"
    echo "Please mount a ConfigMap with sv2-tp.conf"
fi

# Display configuration summary
echo ""
echo "Configuration files:"
echo "  - Bitcoin: /config/bitcoin.conf"
echo "  - SV2-TP:  /config/sv2-tp.conf"
echo ""
echo "Data directory: /data"
echo ""
echo "Starting supervisord..."
echo ""

# Execute the main command (supervisord)
exec "$@"
