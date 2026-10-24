//! Embeds every icon master under `assets/icons/ui/design/svg/` into the binary.

use std::fmt::Write as _;
use std::path::Path;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/icons/ui/design/svg");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut names: Vec<(String, String)> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "svg"))
                .filter_map(|p| {
                    let name = p.file_stem()?.to_str()?.to_owned();
                    let abs = p.canonicalize().ok()?.to_str()?.to_owned();
                    Some((name, abs))
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let mut out = String::from(
        "/// (name, SVG source) for every icon master, sorted by name.\npub static ICONS: &[(&str, &str)] = &[\n",
    );
    for (name, abs) in &names {
        println!("cargo:rerun-if-changed={abs}");
        let _ = writeln!(out, "    ({name:?}, include_str!({abs:?})),");
    }
    out.push_str("];\n");
    let out_dir = std::env::var("OUT_DIR").unwrap_or_else(|_| ".".into());
    if let Err(e) = std::fs::write(Path::new(&out_dir).join("icons.rs"), out) {
        println!("cargo:warning=could not write icons.rs: {e}");
    }
}
