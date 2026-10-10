//! Educational blocking TCP echo server for Phase 1, Week 3, step 1.
//!
//! This example teaches fundamental operating system socket concepts using only
//! the Rust standard library (`std::net`). It demonstrates how sockets behave in
//! synchronous, blocking mode before moving to non-blocking I/O and `epoll`.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};

/// Handles a single client connection sequentially.
///
/// # Sockets Mechanics Explained
///
/// - **`read`**: Copies incoming data from the kernel's socket receive buffer into
///   the application's user-space buffer (`buf`). In blocking mode, `stream.read()`
///   puts the calling thread to sleep until at least 1 byte arrives, or an error occurs.
/// - **Partial Reads**: Sockets are byte streams, not message packets. A single `read`
///   call may return any number of bytes between 1 and `buf.len()`. It is never guaranteed
///   that all data sent by the peer arrives in a single `read` call.
/// - **Client Disconnect (EOF)**: When the client closes its connection or shuts down
///   its writing half, the operating system sends a TCP FIN packet. Upon receiving FIN,
///   `stream.read()` returns `Ok(0)`. This zero-byte read indicates End-Of-File (EOF).
/// - **`write_all`**: Copies bytes into the kernel's socket send buffer. While a raw
///   `stream.write()` can perform a partial write (writing fewer bytes than requested),
///   `write_all()` repeatedly calls write until all `n` bytes have been sent.
///
/// # Errors
///
/// Returns an [`io::Error`] if reading from or writing to the socket fails.
pub fn handle_client(mut stream: TcpStream) -> io::Result<()> {
    // 1024-byte stack buffer for reading incoming chunks.
    let mut buf = [0u8; 1024];

    loop {
        // Blocks until at least 1 byte arrives or client closes the connection.
        let bytes_read = stream.read(&mut buf)?;

        // A read of 0 bytes signals that the client has closed the connection (EOF).
        if bytes_read == 0 {
            break;
        }

        // Echo the exact bytes received back to the client.
        // write_all ensures all bytes_read are written even if the OS writes in chunks.
        stream.write_all(&buf[..bytes_read])?;
    }

    Ok(())
}

/// Runs the blocking echo server loop on a given listener.
///
/// # Sockets Mechanics Explained
///
/// - **`accept`**: In blocking mode, `listener.accept()` puts the thread to sleep until
///   a remote client completes the TCP 3-way handshake (SYN, SYN-ACK, ACK). When a connection
///   arrives, `accept` creates a brand-new socket file descriptor for that specific client,
///   leaving the listening socket free to accept further connections.
/// - **The Blocking Problem**: Because this loop processes one client at a time on a single
///   thread, calling `handle_client(stream)` blocks the entire server. No other client can
///   be accepted or serviced until the currently connected client disconnects!
///
/// # Errors
///
/// Returns an [`io::Error`] if accepting a connection fails.
pub fn run_server(listener: TcpListener) -> io::Result<()> {
    for stream_result in listener.incoming() {
        let stream = stream_result?;
        if let Err(err) = handle_client(stream) {
            eprintln!("Error handling client: {err}");
        }
    }
    Ok(())
}

fn main() -> io::Result<()> {
    // 1. `bind`: Asks the operating system kernel to allocate a socket and associate
    //    it with the given local IP address and port (127.0.0.1:7380).
    // 2. `listen`: Behind the scenes, `TcpListener::bind` marks the socket as passive
    //    (ready to accept incoming connections) and sets up the OS connection queue backlog.
    let listener = TcpListener::bind("127.0.0.1:7380")?;
    println!("Blocking TCP echo server listening on 127.0.0.1:7380");

    run_server(listener)
}
