//! Real end-to-end TCP integration tests for [`cachedb::client::Client`].
//!
//! Evaluates high-throughput pipelining, bulk operations, and multi-threaded
//! concurrent client connections against the Linux epoll event loop server.

#![cfg(target_os = "linux")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
)]

mod common;

use std::thread;

use cachedb::client::Client;
use cachedb::proto::Value;
use common::TestServer;

// 1. connect, 10,000 puts via pipeline in batches of 500, then fetch all
#[test]
fn test_client_e2e_pipeline_10k_puts_and_fetch_all() {
    let server = TestServer::start();
    let mut client = Client::connect(server.addr()).expect("client connect");

    let total = 10_000;
    let batch_size = 500;

    for batch_idx in 0..(total / batch_size) {
        let mut batch = Vec::with_capacity(batch_size);
        for item_idx in 0..batch_size {
            let key = format!("k_{}", batch_idx * batch_size + item_idx);
            let val = Value::Int((batch_idx * batch_size + item_idx) as i64);
            batch.push(vec![
                Value::Text("put".to_string()),
                Value::Bytes(key.into_bytes()),
                val,
            ]);
        }
        let replies = client.pipeline(batch).expect("pipeline batch put");
        assert_eq!(replies.len(), batch_size);
        for reply in replies {
            assert_eq!(reply.expect("put success"), Value::Bool(true));
        }
    }

    assert_eq!(client.size().expect("size"), total as i64);

    // Fetch all 10,000 keys via pipelining in batches of 500
    for batch_idx in 0..(total / batch_size) {
        let mut batch = Vec::with_capacity(batch_size);
        for item_idx in 0..batch_size {
            let key = format!("k_{}", batch_idx * batch_size + item_idx);
            batch.push(vec![
                Value::Text("fetch".to_string()),
                Value::Bytes(key.into_bytes()),
            ]);
        }
        let replies = client.pipeline(batch).expect("pipeline batch fetch");
        assert_eq!(replies.len(), batch_size);
        for (item_idx, reply) in replies.into_iter().enumerate() {
            let expected_val = (batch_idx * batch_size + item_idx) as i64;
            assert_eq!(reply.expect("fetch success"), Value::Int(expected_val));
        }
    }
}

// 2. 20 concurrent clients in threads
#[test]
fn test_client_e2e_20_concurrent_clients() {
    let server = TestServer::start();
    let addr = server.addr();

    let mut handles = Vec::new();
    for client_id in 0..20 {
        let handle = thread::spawn(move || {
            let mut client = Client::connect(addr).expect("client connect in thread");
            assert_eq!(client.ping().expect("ping"), "pong");

            for i in 0..100 {
                let key = format!("thread_{client_id}_key_{i}").into_bytes();
                assert!(client.put(&key, Value::Int(i)).expect("put in thread"));
                assert_eq!(
                    client.fetch(&key).expect("fetch in thread"),
                    Some(Value::Int(i))
                );
            }

            client.quit().expect("quit in thread");
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("thread join");
    }
}
