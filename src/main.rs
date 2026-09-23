mod catalog;
mod cli;
mod engine;
mod handoff;
mod os;
mod system;
mod tui;

use clap::Parser;

fn main() {
    let code = cli::run(cli::Cli::parse());
    cli::hold_window_open();
    std::process::exit(code);
}
