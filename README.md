# MARA Pool Sv2

An implementation of Stratum V2 pool and miner applications 🦀

This repository provides applications using the Stratum V2 protocol, currently in **alpha** stage. This is a separately maintained project that builds upon the foundational work of the Stratum V2 Reference Implementation.

## Attribution

This project is based on the [Stratum V2 Reference Implementation](https://github.com/stratum-mining/sv2-apps) developed by the SRI team. We are deeply grateful for their foundational work on the Stratum V2 protocol and applications. For more information about the original project, visit [stratumprotocol.org](https://stratumprotocol.org).

**This repository is independently maintained and is not officially affiliated with the SRI project.**

For questions about the project direction or to propose new features, please open an issue in this repository or contact foss@mara.com.

## Building and Testing

This repository is organized into multiple workspaces. You can build and test all workspaces or work with individual ones.

**Build all workspaces:**
```bash
./scripts/build-all-workspaces.sh
```

**Run tests, clippy, and formatter across all workspaces:**
```bash
./scripts/clippy-fmt-and-test.sh
```

**Or run commands individually in each workspace:**
```bash
cargo test    # Run test suite
cargo clippy  # Check for common mistakes and style issues
cargo fmt     # Format code according to style guidelines
```

## Contents

- `bitcoin-core-sv2/` - Library crate that translates Bitcoin Core IPC into Sv2 Template Distribution Protocol
- `pool-apps/` - Pool operator applications
  - `pool/` - SV2-compatible mining pool server that communicates with downstream roles and Template Providers
  - `jd-server/` - Job Declarator Server coordinates job declaration between miners and pools, maintains synchronized mempool
- `miner-apps/` - Miner applications
  - `jd-client/` - Job Declarator Client allows miners to declare custom block templates for decentralized mining
  - `translator/` - Translator Proxy bridges SV1 miners to SV2 pools, enabling protocol transition
  - `mining-device/` - Mining device simulator for development and testing
- `stratum-apps/` - Shared application utilities
  - Configuration helpers (TOML, coinbase outputs, logging)
  - Network connection utilities (Noise protocol, plain TCP, SV1 connections)
  - RPC client implementation
  - Key management utilities
  - Custom synchronization primitives
- `integration-tests/` - End-to-end integration tests validating interoperability between all components

## Contributing

We welcome contributions! Here's how to get started:

1. **Fork the repository** and create a branch for your changes
2. **Make your changes** following Rust best practices
3. **Run tests**: Use `./scripts/clippy-fmt-and-test.sh` to run tests, clippy, and formatter across all workspaces
4. **Submit a pull request** with a clear description of your changes

For questions or discussions, please open an issue in this repository or contact foss@mara.com

## 📖 License

This software is licensed under Apache 2.0 or MIT, at your option.

## 🦀 MSRV

Minimum Supported Rust Version: 1.85.0
