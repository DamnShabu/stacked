//! Point a bundled `libwebkitgtk-6.0.so` at the directory Stacked stages its
//! helper processes in. Run by `packaging/appimage/build-appimage.sh`; see
//! `cordial_shell::webkit_helpers` for why, and ADR-045 for the decision.
//!
//! Usage: `patch_webkit_helper_dir <library> <baked-dir> <hex-key>`. Prints the
//! staging directory the library now names, which the build writes to the
//! sidecar the client reads at runtime, and fails without touching the file
//! when the baked directory is not there to patch exactly.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, lib, baked, key] = args.as_slice() else {
        eprintln!("usage: patch_webkit_helper_dir <library> <baked-dir> <hex-key>");
        std::process::exit(2);
    };
    match cordial_shell::webkit_helpers::patch_library_file(std::path::Path::new(lib), baked, key) {
        Ok((dir, count)) => {
            eprintln!("patched {count} string(s) in {lib}: {baked} -> {dir}");
            println!("{dir}");
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
