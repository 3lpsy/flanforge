use std::path::Path;

/// The UI bundle is embedded from `webui/dist`. A build without one — every
/// backend-only workflow — must still compile, so the directory is created
/// empty here and the server answers 503 for `/ui` until `just ui-build`.
/// A committed `.gitkeep` backstops this: a cached build-script fingerprint
/// can skip this script entirely on a fresh checkout (it did, in CI).
fn main() {
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../webui/dist");
    let _ = std::fs::create_dir_all(&dist);
    println!("cargo:rerun-if-changed={}", dist.display());
}
