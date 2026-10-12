//! End-to-end integration tests for SkiffDB.

// AGENTS.md Section 5: unwrap and expect are explicitly allowed in tests.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::thread;

#[allow(dead_code)]
#[path = "../examples/echo_blocking.rs"]
mod echo_blocking;

#[test]
fn test_blocking_echo_server_round_trip() -> Result<(), Box<dyn std::error::Error>> {
    // Bind to port 0 so the operating system assigns a random unused port.
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;

    // Run the server handler for a single client on a background thread.
    let server_handle = thread::spawn(move || -> std::io::Result<()> {
        let (stream, _) = listener.accept()?;
        echo_blocking::handle_client(stream)
    });

    // Connect from the test client.
    let mut client = TcpStream::connect(addr)?;
    client.set_read_timeout(Some(std::time::Duration::from_secs(3)))?;
    client.set_write_timeout(Some(std::time::Duration::from_secs(3)))?;

    // Send payload 1.
    let message_1 = b"hello, skiffdb blocking echo server!";
    client.write_all(message_1)?;
    let mut response_1 = vec![0u8; message_1.len()];
    client.read_exact(&mut response_1)?;
    assert_eq!(&response_1[..], message_1);

    // Send payload 2 over the same open connection.
    let message_2 = b"second message over same connection";
    client.write_all(message_2)?;
    let mut response_2 = vec![0u8; message_2.len()];
    client.read_exact(&mut response_2)?;
    assert_eq!(&response_2[..], message_2);

    // Close the connection from the client side.
    client.shutdown(Shutdown::Both)?;
    drop(client);

    // Verify the server finishes cleanly upon EOF without errors.
    let server_res = server_handle
        .join()
        .map_err(|_| std::io::Error::other("server thread panicked"))?;
    server_res?;

    Ok(())
}

#[allow(dead_code)]
#[path = "../examples/echo_nonblocking.rs"]
mod echo_nonblocking;

#[cfg(target_os = "linux")]
#[test]
fn test_nonblocking_echo_server_creation() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let server = echo_nonblocking::NonBlockingEchoServer::new(listener)?;
    assert_eq!(server.client_count(), 0);
    Ok(())
}

#[cfg(not(target_os = "linux"))]
#[test]
fn test_nonblocking_echo_server_creation_unsupported_on_non_linux()
-> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let res = echo_nonblocking::NonBlockingEchoServer::new(listener);
    assert!(res.is_err());
    assert_eq!(res.unwrap_err().kind(), std::io::ErrorKind::Unsupported);
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn test_nonblocking_echo_server_concurrent_clients() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;

    let mut server = echo_nonblocking::NonBlockingEchoServer::new(listener)?;

    // Run the non-blocking event loop on a background thread for a limited number of ticks.
    let server_handle = thread::spawn(move || -> std::io::Result<()> {
        server.run_ticks(100, Some(std::time::Duration::from_millis(20)))
    });

    // Client 1 connects and sends payload.
    let mut client1 = TcpStream::connect(addr)?;
    client1.set_read_timeout(Some(std::time::Duration::from_secs(3)))?;
    client1.set_write_timeout(Some(std::time::Duration::from_secs(3)))?;
    let msg1 = b"hello from client 1";
    client1.write_all(msg1)?;

    // Client 2 connects concurrently while Client 1 is still connected!
    let mut client2 = TcpStream::connect(addr)?;
    client2.set_read_timeout(Some(std::time::Duration::from_secs(3)))?;
    client2.set_write_timeout(Some(std::time::Duration::from_secs(3)))?;
    let msg2 = b"hello from client 2 concurrent";
    client2.write_all(msg2)?;

    // Verify Client 1 receives its full echo.
    let mut resp1 = vec![0u8; msg1.len()];
    client1.read_exact(&mut resp1)?;
    assert_eq!(&resp1[..], msg1);

    // Verify Client 2 receives its full echo without being blocked by Client 1.
    let mut resp2 = vec![0u8; msg2.len()];
    client2.read_exact(&mut resp2)?;
    assert_eq!(&resp2[..], msg2);

    // Orderly disconnect.
    drop(client1);
    drop(client2);

    let _ = server_handle.join();
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn test_event_loop_shutdown_via_signal_flag() -> Result<(), Box<dyn std::error::Error>> {
    use skiffdb::net::event_loop::{EventLoop, Handler, Token};
    use skiffdb::net::sys::{EpollFlags, is_shutdown_requested, request_shutdown, reset_shutdown};

    struct NoopHandler;
    impl Handler for NoopHandler {
        fn on_event(
            &mut self,
            _event_loop: &mut EventLoop,
            _token: Token,
            _flags: EpollFlags,
        ) -> std::io::Result<()> {
            Ok(())
        }
    }

    reset_shutdown();
    assert!(!is_shutdown_requested());

    let mut event_loop = EventLoop::new()?;
    let mut handler = NoopHandler;

    // Simulate signal arrival by requesting shutdown.
    request_shutdown();
    assert!(is_shutdown_requested());

    // EventLoop::run should observe the shutdown flag immediately and stop.
    event_loop.run(&mut handler)?;
    assert!(!event_loop.is_running());

    reset_shutdown();
    assert!(!is_shutdown_requested());
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn test_connection_table_backpressure_interest_toggling() -> Result<(), Box<dyn std::error::Error>>
{
    use skiffdb::net::connection::ConnectionTable;
    use skiffdb::net::event_loop::EventLoop;
    use skiffdb::net::sys::EpollFlags;

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;

    let mut event_loop = EventLoop::new()?;
    let mut table = ConnectionTable::with_max_connections(10);

    // Create client connection.
    let client_stream = TcpStream::connect(addr)?;
    let (server_stream, peer_addr) = listener.accept()?;

    let token = table.insert(server_stream, peer_addr)?;
    table.register_with_loop(token, &mut event_loop)?;

    // Initial state: not backpressured -> desired_flags has READABLE, not WRITABLE.
    {
        let conn = table.get(token).expect("connection exists");
        assert_eq!(conn.desired_flags(), EpollFlags::READABLE);
    }

    // Enqueue large payload to exceed high watermark (10 MB).
    {
        let conn = table.get_mut(token).expect("connection exists");
        let large_payload = vec![0u8; 11 * 1024 * 1024];
        conn.send_chunk(large_payload)?;

        // Backpressure triggered: READABLE removed, WRITABLE added!
        assert_eq!(conn.desired_flags(), EpollFlags::WRITABLE);
        assert!(conn.is_backpressured());
    }

    // Sync interest with event loop.
    table.sync_interest(token, &mut event_loop)?;

    // Clean up.
    table.remove_and_deregister(token, &mut event_loop);
    drop(client_stream);
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn test_connection_table_capacity_and_shutdown_drain() -> Result<(), Box<dyn std::error::Error>> {
    use skiffdb::net::connection::ConnectionTable;
    use skiffdb::net::event_loop::EventLoop;
    use skiffdb::net::sys::EpollFlags;

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;

    let mut event_loop = EventLoop::new()?;
    let mut table = ConnectionTable::with_max_connections(2);

    // Connect client 1 and client 2.
    let client1 = TcpStream::connect(addr)?;
    let (server1, peer1) = listener.accept()?;
    let token1 = table.insert(server1, peer1)?;
    table.register_with_loop(token1, &mut event_loop)?;

    let client2 = TcpStream::connect(addr)?;
    let (server2, peer2) = listener.accept()?;
    let token2 = table.insert(server2, peer2)?;
    table.register_with_loop(token2, &mut event_loop)?;

    assert!(table.is_at_capacity());
    assert_eq!(table.remaining_capacity(), 0);

    // Attempt client 3: rejected with ConnectionRefused.
    let client3 = TcpStream::connect(addr)?;
    let (server3, peer3) = listener.accept()?;
    let res3 = table.insert(server3, peer3);
    assert!(res3.is_err());
    assert_eq!(
        res3.unwrap_err().kind(),
        std::io::ErrorKind::ConnectionRefused
    );

    // Queue response bytes into client 1.
    {
        let conn1 = table.get_mut(token1).expect("client 1 exists");
        conn1.send_slice(b"queued response")?;
    }

    // Initiate shutdown:
    // Client 2 has empty write queue -> immediately closed and deregistered.
    // Client 1 has pending bytes -> marked closing, keeps WRITABLE.
    table.initiate_shutdown(&mut event_loop);

    assert_eq!(table.len(), 1);
    assert!(!table.contains(token2));
    assert!(table.contains(token1));

    {
        let conn1 = table.get_mut(token1).expect("client 1 exists");
        assert!(conn1.is_closing());
        assert_eq!(conn1.desired_flags(), EpollFlags::WRITABLE);
        // Flush client 1.
        let status = conn1.flush_outgoing()?;
        assert_eq!(status, skiffdb::net::connection::WriteStatus::Complete);
        assert!(conn1.is_closed());
    }

    // Sync interest on closed connection -> deregisters.
    table.sync_interest(token1, &mut event_loop)?;
    table.remove(token1);
    assert!(table.is_empty());

    drop(client1);
    drop(client2);
    drop(client3);
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn test_nonblocking_echo_server_limits_and_idle_timeout() -> Result<(), Box<dyn std::error::Error>>
{
    use std::time::Duration;

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;

    let mut server = echo_nonblocking::NonBlockingEchoServer::with_config(
        listener,
        2,
        Some(Duration::from_millis(80)),
        Duration::from_millis(40),
        Duration::from_secs(1),
    )?;

    // Run ticks in background thread.
    let server_handle = thread::spawn(move || -> std::io::Result<()> {
        server.run_ticks(100, Some(Duration::from_millis(20)))
    });

    // Connect client 1 and client 2.
    let mut client1 = TcpStream::connect(addr)?;
    client1.set_read_timeout(Some(Duration::from_secs(3)))?;
    client1.set_write_timeout(Some(Duration::from_secs(3)))?;
    let mut client2 = TcpStream::connect(addr)?;
    client2.set_read_timeout(Some(Duration::from_secs(3)))?;
    client2.set_write_timeout(Some(Duration::from_secs(3)))?;

    // Send data on client 1.
    client1.write_all(b"ping")?;
    let mut buf = [0u8; 4];
    client1.read_exact(&mut buf)?;
    assert_eq!(&buf, b"ping");

    // Connect client 3: should be rejected by server.
    let client3 = TcpStream::connect(addr)?;
    client3.set_read_timeout(Some(Duration::from_secs(3)))?;
    client3.set_write_timeout(Some(Duration::from_secs(3)))?;
    // Sleep briefly to let server sweep and disconnect idle client 2.
    thread::sleep(Duration::from_millis(150));

    // Client 2 was idle for > 80ms, so server closed it.
    let mut eof_buf = [0u8; 16];
    let n = client2.read(&mut eof_buf)?;
    assert_eq!(
        n, 0,
        "idle client 2 should have been closed by idle sweeper"
    );

    drop(client1);
    drop(client2);
    drop(client3);
    let _ = server_handle.join();
    Ok(())
}
