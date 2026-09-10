mod catalog;
mod cli;
mod engine;
mod os;
mod system;
mod tui;

use clap::Parser;

fn main() {
    let code = cli::run(cli::Cli::parse());
    std::process::exit(code);
}
