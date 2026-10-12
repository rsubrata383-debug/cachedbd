//! End-to-end integration tests for the `cachedb-bench` load generator.

#![cfg(target_os = "linux")]

mod common;

use cachedb::bench::{BenchConfig, Workload, run_benchmark};
use common::TestServer;

/// Tests the benchmark load generator across all four standard workloads (`read`, `write`,
/// `mixed`, `bump`) against a live [`TestServer`].
///
/// In the `mixed` workload, `bump` operations target a separate counter key space (`cnt:<idx>`),
/// ensuring atomic increments never collide with keys holding string or byte payloads from `put`
/// commands. Without this key space separation, `bump` would encounter non-integer stored values
/// and elicit error code 3004 (`ERR_NOT_AN_INTEGER`).
#[test]
fn test_bench_e2e_all_workloads() {
    let server = TestServer::start();
    let addr = server.addr().to_string();

    let workloads = [
        Workload::Read,
        Workload::Write,
        Workload::Mixed,
        Workload::Bump,
    ];

    for workload in workloads {
        let config = BenchConfig {
            addr: addr.clone(),
            connections: 2,
            duration: 1,
            warmup: 0,
            pipeline: 4,
            keys: 200,
            value_size: 32,
            workload,
            seed: 12345,
            preload: true,
            json: true,
            show_help: false,
        };

        let report = run_benchmark(&config).unwrap_or_else(|e| {
            panic!("benchmark run failed for workload {}: {e}", workload.name())
        });

        assert!(
            report.total_ops > 0,
            "workload {} produced zero operations",
            workload.name()
        );
        assert_eq!(
            report.total_errors,
            0,
            "workload {} produced {} errors: {:?}",
            workload.name(),
            report.total_errors,
            report.thread_errors
        );
        assert!(
            !report.has_failures(),
            "workload {} reported thread failures: {:?}",
            workload.name(),
            report.thread_errors
        );
        assert_ne!(report.to_json(), "");
    }
}
