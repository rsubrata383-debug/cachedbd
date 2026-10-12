//! Main entry point for the cachedbd server binary.

use std::env;
use std::net::TcpListener;
use std::process;

fn print_help() {
    println!("Usage: cachedbd [OPTIONS]");
    println!();
    println!("Options:");
    println!("  --bind <ADDR>    IP address to bind to (default: 127.0.0.1)");
    println!("  --port <PORT>    TCP port to listen on (default: 7878)");
    println!("  -h, --help       Print help information");
}

fn parse_cli_args() -> (String, u16) {
    let mut args = env::args().skip(1);
    let mut bind = "127.0.0.1".to_string();
    let mut port: u16 = 7878;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_help();
                process::exit(0);
            }
            "--bind" => {
                let Some(val) = args.next() else {
                    eprintln!("error: missing argument for --bind");
                    process::exit(2);
                };
                bind = val;
            }
            "--port" => {
                let Some(val) = args.next() else {
                    eprintln!("error: missing argument for --port");
                    process::exit(2);
                };
                match val.parse::<u16>() {
                    Ok(p) if p > 0 => port = p,
                    _ => {
                        eprintln!("error: invalid port number: '{val}'");
                        process::exit(2);
                    }
                }
            }
            other => {
                eprintln!("error: unrecognized argument: '{other}'");
                process::exit(2);
            }
        }
    }

    (bind, port)
}

fn main() {
    let (bind, port) = parse_cli_args();
    let addr_str = format!("{bind}:{port}");

    let listener = match TcpListener::bind(&addr_str) {
        Ok(l) => l,
        Err(err) => {
            eprintln!("error: failed to bind to {addr_str}: {err}");
            process::exit(1);
        }
    };

    println!("cachedbd 0.1.0 listening on {addr_str}");

    #[cfg(target_os = "linux")]
    {
        use cachedb::net::event_loop::EventLoop;
        use cachedb::net::sys::{EpollFlags, as_raw_fd};
        use cachedb::server::{LISTENER_TOKEN, Server};

        let mut event_loop = match EventLoop::new() {
            Ok(el) => el,
            Err(err) => {
                eprintln!("error: failed to initialize event loop: {err}");
                process::exit(1);
            }
        };

        if let Err(err) =
            event_loop.register(as_raw_fd(&listener), LISTENER_TOKEN, EpollFlags::READABLE)
        {
            eprintln!("error: failed to register listener socket: {err}");
            process::exit(1);
        }

        let mut server = Server::new(listener);
        if let Err(err) = event_loop.run(&mut server) {
            eprintln!("error: event loop error: {err}");
            process::exit(1);
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        drop(listener);
    }
}
