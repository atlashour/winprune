mod catalog;
mod engine;
mod os;
mod system;

fn main() {
    let info = os::detect();
    match catalog::Catalog::embedded() {
        Ok(c) => println!(
            "winprune {} ({} catalog items, build {}, elevated {})",
            env!("CARGO_PKG_VERSION"),
            c.items.len(),
            info.build,
            info.elevated
        ),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
