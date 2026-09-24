mod catalog;
mod cli;
mod engine;
mod handoff;
mod os;
mod system;
mod tui;

use clap::Parser;

fn main() {
    std::process::exit(cli::run(cli::Cli::parse()));
}
