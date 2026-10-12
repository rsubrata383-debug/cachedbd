//! Command-line client application for SkiffDB.

use std::env;
use std::io::IsTerminal;
use std::process;

use skiffdb::cli::{HELP_TEXT, parse_args, run};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let config = match parse_args(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            process::exit(2);
        }
    };

    if config.show_help {
        println!("{HELP_TEXT}");
        process::exit(0);
    }

    if config.show_version {
        println!("skiff-cli {}", env!("CARGO_PKG_VERSION"));
        process::exit(0);
    }

    let stdin = std::io::stdin();
    let is_terminal = stdin.is_terminal();
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    let code = run(config, stdin.lock(), &mut stdout, &mut stderr, is_terminal);
    process::exit(code);
}
