//! Command-line benchmark load generator binary for cachedb.

use std::env;
use std::process;

use cachedb::bench::{HELP_TEXT, parse_args, run_benchmark};

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

    match run_benchmark(&config) {
        Ok(report) => {
            if let Err(e) = report.print_table(&mut std::io::stdout()) {
                eprintln!("error printing benchmark report: {e}");
                process::exit(1);
            }
            if config.json {
                println!("{}", report.to_json());
            }
            if report.has_failures() {
                process::exit(1);
            }
            process::exit(0);
        }
        Err(err) => {
            eprintln!("error: {err}");
            process::exit(1);
        }
    }
}
