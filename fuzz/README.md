# Cachedb Fuzzing

Fuzz testing targets for the Cachedb binary wire protocol using `cargo-fuzz` (LLVM libFuzzer).

> **Platform Notice**: `cargo-fuzz` and LLVM libFuzzer require a Linux or macOS environment.
> They are **not supported on Windows**. To run fuzzing locally on Windows, use WSL2 or a Linux dev container / GitHub Codespaces, or rely on the automated GitHub Actions CI workflow.

## Prerequisites (Linux / WSL)

Install a nightly Rust toolchain and `cargo-fuzz`:

```bash
rustup toolchain install nightly
cargo +nightly install cargo-fuzz --locked
```

## Running Fuzz Targets

Run any target from the repository root:

```bash
# Fuzz full frame decode/encode round trips
cargo +nightly fuzz run decode_frame -- -max_total_time=60

# Fuzz typed payload item decode/encode round trips
cargo +nightly fuzz run decode_value -- -max_total_time=60

# Fuzz chunked stream reassembly across arbitrary split points
cargo +nightly fuzz run split_stream -- -max_total_time=60
```

Seeds for initial corpus exploration are located in `fuzz/corpus/decode_frame/` and `fuzz/corpus/decode_value/`.

