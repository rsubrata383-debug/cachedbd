# Cachedb

Cachedb is a fast, durable, in-memory key-value data store built from scratch in Rust. It has its own binary wire protocol, command language, storage formats, and tooling. It is designed for caches, session stores, queues, and leaderboards.

## Current Status

Phase 0 done, specs v0.1 frozen. Phase 1 not started.

## Primary Operating System

The primary operating system is Linux (use WSL2 on Windows). Linux kernel APIs (`epoll`, non-blocking TCP) are the main networking target.

## Binaries and Port

- **Server**: `cachedbd`
- **Client**: `cachedb-cli`
- **Benchmark Tool**: `cachedb-bench`
- **Default Port**: `7379`

## How to Build and Check

Run these four commands to build and check the code:

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo doc --no-deps
```

## Documentation

- **Project Roadmap**: [cachedb-roadmap.md](cachedb-roadmap.md)
- **Design Journal**: [docs/journal.md](docs/journal.md)
- **Design Docs**: [docs/design/](docs/design/)
  - [Vision and Goals](docs/design/vision.md)
  - [Naming Choices](docs/design/naming.md)
  - [Threading Model](docs/design/threading.md)
- **Specifications (v0.1 Frozen)**: [docs/spec/](docs/spec/)
  - [Wire Protocol](docs/spec/protocol.md)
  - [Value Model](docs/spec/values.md)
  - [Error Codes](docs/spec/errors.md)
  - [Commands](docs/spec/commands.md)
