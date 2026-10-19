//! Runs the real command line through `snap_engine::cli::run`:
//! `cargo run -p snap-engine --example cli -- capture --monitor 0 --out shot.png`.
//!
//! GUI-bound commands are reported instead of executed, since no app is running here.

use snap_engine::cli::{self, Dispatch};

fn main() {
    let cli = cli::parse();
    let code = match cli::dispatch(&cli) {
        Dispatch::Gui(request) => {
            eprintln!("would hand this to the app: {request:?}");
            0
        }
        Dispatch::Headless => match cli::run(cli) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
    };
    std::process::exit(code);
}
