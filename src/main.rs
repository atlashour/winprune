mod catalog;

fn main() {
    match catalog::Catalog::embedded() {
        Ok(c) => println!(
            "winprune {} ({} catalog items)",
            env!("CARGO_PKG_VERSION"),
            c.items.len()
        ),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
