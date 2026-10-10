# Cachedb Naming Choices and Conventions

This document records the finalized naming conventions, file formats, networking settings, and target platform for Cachedb.

---

## 1. Project Name

- **Name**: `cachedb`

---

## 2. Binaries

- **Server Binary**: `cachedbd`
  - Path: `src/main.rs`
  - Defined in `Cargo.toml` as `[[bin]] name = "cachedbd"`.
  - The trailing `d` clearly identifies the server daemon.
- **Client Shell Binary**: `cachedb-cli`
  - Path: `src/bin/cachedb-cli.rs`
  - Interactive command-line client.
- **Benchmark Binary**: `cachedb-bench`
  - Path: `src/bin/cachedb-bench.rs`
  - Load generation and performance benchmarking tool.

---

## 3. Storage File Extensions

- **Write-Ahead Journal (WAL)**: `.cdbj` (Cachedb Journal)
- **Snapshot File**: `.cdbs` (Cachedb Snapshot)
- **Configuration File**: `cachedb.toml` (TOML format)

---

## 4. Networking

- **Default Port**: `7379`
  - Easy to remember, distinctive port inspired by standard cache services.

---

## 5. Primary Operating System

- **Primary OS**: Linux (developed using WSL2 / Ubuntu).
- Linux kernel APIs (`epoll`, non-blocking sockets) are the primary networking target, with abstractions kept clean for portability.
