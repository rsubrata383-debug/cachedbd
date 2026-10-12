# SkiffDB Naming Choices and Conventions

This document records the finalized naming conventions, file formats, networking settings, and target platform for SkiffDB.

---

## 1. Project Name

- **Name**: `skiffdb` (display name: **SkiffDB**)

---

## 2. Binaries

- **Server Binary**: `skiffd`
  - Path: `src/main.rs`
  - Defined in `Cargo.toml` as `[[bin]] name = "skiffd"`.
  - The trailing `d` clearly identifies the server daemon.
- **Client Shell Binary**: `skiff-cli`
  - Path: `src/bin/skiff-cli.rs`
  - Interactive command-line client.
- **Benchmark Binary**: `skiff-bench`
  - Path: `src/bin/skiff-bench.rs`
  - Load generation and performance benchmarking tool.

---

## 3. Storage File Extensions

- **Write-Ahead Journal (WAL)**: `.skfj` (SkiffDB Journal)
- **Snapshot File**: `.skfs` (SkiffDB Snapshot)
- **Configuration File**: `skiff.toml` (TOML format)

---

## 4. Networking

- **Default Port**: `7878` (configured as standard default in `skiffd` and `skiff-cli`).
- **Wire Protocol Magic**: `0xCA 0xDB` (`0xCADB`).

---

## 5. Primary Operating System

- **Primary OS**: Linux (developed using WSL2 / Ubuntu).
- Linux kernel APIs (`epoll`, non-blocking sockets) are the primary networking target for `skiffd`, while `skiff-cli` and client libraries build and run cross-platform on Linux and Windows.
