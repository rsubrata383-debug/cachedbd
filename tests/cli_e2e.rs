//! End-to-end integration tests for `skiff-cli`.

mod common;

use std::io::Write;
use std::net::TcpListener;
use std::process::{Command, Stdio};

use common::TestBlockingServer;

#[test]
fn test_cli_help_and_version() {
    let help = Command::new(env!("CARGO_BIN_EXE_skiff-cli"))
        .arg("--help")
        .output()
        .expect("run --help");
    assert_eq!(help.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(stdout.contains("skiff-cli [OPTIONS]"));

    let version = Command::new(env!("CARGO_BIN_EXE_skiff-cli"))
        .arg("--version")
        .output()
        .expect("run --version");
    assert_eq!(version.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&version.stdout);
    assert!(stdout.contains("skiff-cli 0.1.0"));
}

#[test]
fn test_cli_usage_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_skiff-cli"))
        .arg("--invalid-flag")
        .output()
        .expect("run with invalid flag");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("usage error: unknown option: '--invalid-flag'"));
}

#[test]
fn test_cli_connection_error_closed_port() {
    let closed_port = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("addr").port()
    };

    let output = Command::new(env!("CARGO_BIN_EXE_skiff-cli"))
        .args(["--port", &closed_port.to_string(), "ping"])
        .output()
        .expect("run against closed port");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("connection error:"));
}

#[test]
fn test_cli_one_shot_commands() {
    let server = TestBlockingServer::start();
    let port = server.addr().port().to_string();

    // 1. put foo bar -> exit 0, stdout has (bool) true
    let put_out = Command::new(env!("CARGO_BIN_EXE_skiff-cli"))
        .args(["--host", "127.0.0.1", "--port", &port, "put", "foo", "bar"])
        .output()
        .expect("run put");
    assert_eq!(put_out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&put_out.stdout);
    assert!(stdout.contains("(bool) true"));

    // 2. fetch foo -> exit 0, stdout has "bar"
    let fetch_out = Command::new(env!("CARGO_BIN_EXE_skiff-cli"))
        .args(["--host", "127.0.0.1", "--port", &port, "fetch", "foo"])
        .output()
        .expect("run fetch");
    assert_eq!(fetch_out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&fetch_out.stdout);
    assert_eq!(stdout.trim(), "\"bar\"");

    // 3. unknown_cmd -> exit 1, stderr has (error 2001) unknown command
    let unknown_out = Command::new(env!("CARGO_BIN_EXE_skiff-cli"))
        .args(["--host", "127.0.0.1", "--port", &port, "unknown_cmd"])
        .output()
        .expect("run unknown_cmd");
    assert_eq!(unknown_out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&unknown_out.stderr);
    assert!(stderr.contains("(error 2001) unknown command"));

    // 4. bump against text value -> exit 1, stderr has (error 3004) value is not an integer
    let bump_out = Command::new(env!("CARGO_BIN_EXE_skiff-cli"))
        .args(["--host", "127.0.0.1", "--port", &port, "bump", "foo"])
        .output()
        .expect("run bump");
    assert_eq!(bump_out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&bump_out.stderr);
    assert!(stderr.contains("(error 3004) value is not an integer"));
}

#[test]
fn test_cli_interactive_piped() {
    let server = TestBlockingServer::start();
    let port = server.addr().port().to_string();

    let mut child = Command::new(env!("CARGO_BIN_EXE_skiff-cli"))
        .args(["--host", "127.0.0.1", "--port", &port])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cli");

    let mut stdin = child.stdin.take().expect("stdin handle");
    stdin
        .write_all(b"ping\nput a 1\nfetch a\nsize\nexit\n")
        .expect("write commands");
    drop(stdin);

    let output = child.wait_with_output().expect("wait for child");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"pong\""));
    assert!(stdout.contains("(bool) true"));
    assert!(stdout.contains("(integer) 1"));
}

#[test]
fn test_cli_interactive_continues_after_errors() {
    let server = TestBlockingServer::start();
    let port = server.addr().port().to_string();

    let mut child = Command::new(env!("CARGO_BIN_EXE_skiff-cli"))
        .args(["--host", "127.0.0.1", "--port", &port])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cli");

    let mut stdin = child.stdin.take().expect("stdin handle");
    // Invalid syntax (unterminated quote), unknown server command, then valid commands
    stdin
        .write_all(b"\"unterminated\nunknown_cmd\nping\nput x 42\nfetch x\nexit\n")
        .expect("write commands");
    drop(stdin);

    let output = child.wait_with_output().expect("wait for child");
    assert_eq!(output.status.code(), Some(0));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unterminated quote"));
    assert!(stderr.contains("(error 2001) unknown command"));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"pong\""));
    assert!(stdout.contains("(integer) 42"));
}

#[test]
fn test_cli_interactive_server_disconnects() {
    use skiffdb::proto::decode::decode_frame;
    use skiffdb::proto::encode::encode_frame;
    use skiffdb::proto::header::{FLAG_RESPONSE, Header, Opcode};
    use skiffdb::proto::{Decoded, Frame};

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let port = listener.local_addr().expect("addr").port().to_string();

    let server_thread = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept connection");
        // Read full client HELLO frame
        let mut buf = Vec::new();
        let mut temp = [0u8; 1024];
        loop {
            match std::io::Read::read(&mut stream, &mut temp) {
                Ok(n) if n > 0 => {
                    buf.extend_from_slice(&temp[..n]);
                    if let Ok(Decoded::Complete((_, used))) = decode_frame(&buf) {
                        buf.drain(..used);
                        break;
                    }
                }
                _ => break,
            }
        }

        // Send HELLO response
        let resp_header = Header::new(FLAG_RESPONSE, 1, Opcode::Hello, 0);
        let resp_val = skiffdb::proto::hello::HelloResponse::new(
            "test-server".to_string(),
            "0.1.0".to_string(),
            1,
            1,
        )
        .to_value();
        let mut resp_bytes = Vec::new();
        encode_frame(&Frame::new(resp_header, resp_val), &mut resp_bytes).expect("encode hello");
        stream.write_all(&resp_bytes).expect("send hello response");

        // Now drop the connection immediately
        drop(stream);
    });

    let mut child = Command::new(env!("CARGO_BIN_EXE_skiff-cli"))
        .args(["--host", "127.0.0.1", "--port", &port])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cli");

    let mut stdin = child.stdin.take().expect("stdin handle");
    // Send a command on dropped connection
    let _ = stdin.write_all(b"ping\n");
    drop(stdin);

    let output = child.wait_with_output().expect("wait for child");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr.contains("connection closed by server"));

    server_thread.join().expect("join server");
}
