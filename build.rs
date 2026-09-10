use embed_manifest::{embed_manifest, new_manifest};

// Defaults of new_manifest are exactly what we want: asInvoker (plan and dry-run must
// not trigger UAC; apply elevates on demand), UTF-8 code page, long paths, Windows 10+.
fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_manifest(new_manifest("Winprune")).expect("embedding the manifest");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
