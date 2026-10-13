mod catalog;
mod cli;
mod engine;
mod handoff;
mod os;
mod system;
mod tui;

use clap::Parser;

fn main() {
    // clap exits with 2 on a bad argument, which here means "some operations failed"
    let parsed = match cli::Cli::try_parse() {
        Ok(parsed) => parsed,
        Err(e) => {
            let _ = e.print();
            std::process::exit(if e.use_stderr() {
                cli::EXIT_USAGE
            } else {
                cli::EXIT_OK
            });
        }
    };
    std::process::exit(cli::run(parsed));
}
