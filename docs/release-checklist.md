# Release Checklist (v0.1.0)

Checklist for maintainers when cutting a release.

---

## Pre-Release Verification

- [x] **CI Passing**: Verify all CI jobs are green on the release commit (formatting, Clippy, cross-compilation, unit tests, doc tests, integration tests, fuzz checks, and sanitizers).
- [x] **Network Benchmark Workflow**: Manually trigger or review `.github/workflows/network-bench.yml` on GitHub Actions to ensure network benchmark metrics are within expected thresholds.
- [x] **Documentation Review**: Check [`README.md`](../README.md) and [`CHANGELOG.md`](../CHANGELOG.md) to ensure all features, command syntax, and known limitations match the codebase.
- [x] **Roadmap Review**: Inspect `git diff skiffdb-roadmap.md` and tick completed Phase 1 milestone checkboxes by hand.
- [x] **License Check**: Dual license files ([`LICENSE-MIT`](../LICENSE-MIT) and [`LICENSE-APACHE`](../LICENSE-APACHE)) exist in the repository root (matching `license = "MIT OR Apache-2.0"` in `Cargo.toml`).

---

## Tagging & Publishing

- [ ] **Create Annotated Tag**:
  ```bash
  git tag -a v0.1.0 -m "skiffdb v0.1.0"
  ```
- [ ] **Push Tag**:
  ```bash
  git push origin v0.1.0
  ```
- [ ] **GitHub Release** _(Optional)_: Create a GitHub release from tag `v0.1.0` using the release summary from [`CHANGELOG.md`](../CHANGELOG.md).
