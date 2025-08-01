# Proxy Server

High-performance async TCP proxy server written in Rust.

## Usage

```bash
# Basic usage - proxy to target IP
cargo run -- <TARGET_IP>

# Custom configuration
cargo run -- 192.168.1.100 --listen-port 8080 --target-port 443 --max-connections 500
```

## Build

```bash
# Development build
cargo build

# Release build
cargo build --release

# Static build for Linux x86_64
cargo build --release --target x86_64-unknown-linux-musl
```

## Options

- `--listen-ip`: Listen IP address (default: 0.0.0.0)
- `--listen-port`: Listen port (default: 9999)
- `--target-port`: Target port (default: 80)
- `--max-connections`: Maximum concurrent connections (default: 1000)
- `--timeout`: Connection timeout in seconds (default: 30)
- `--log-level`: Log level (trace, debug, info, warn, error)