//! End-to-end integration tests for cachedb.

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

    // Send payload 1.
    let message_1 = b"hello, cachedb blocking echo server!";
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

#[test]
fn test_nonblocking_echo_server_creation() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let server = echo_nonblocking::NonBlockingEchoServer::new(listener)?;
    assert_eq!(server.client_count(), 0);
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
    let msg1 = b"hello from client 1";
    client1.write_all(msg1)?;

    // Client 2 connects concurrently while Client 1 is still connected!
    let mut client2 = TcpStream::connect(addr)?;
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

#[test]
fn test_event_loop_shutdown_via_signal_flag() -> Result<(), Box<dyn std::error::Error>> {
    use cachedb::net::event_loop::{EventLoop, Handler, Token};
    use cachedb::net::sys::{EpollFlags, is_shutdown_requested, request_shutdown, reset_shutdown};

    reset_shutdown();
    assert!(!is_shutdown_requested());

    struct NoopHandler;
    impl Handler for NoopHandler {
        fn on_event(&mut self, _token: Token, _flags: EpollFlags) -> std::io::Result<()> {
            Ok(())
        }
    }

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
