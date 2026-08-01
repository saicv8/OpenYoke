fn main() {
    // `tauri_build` embeds the icons at compile time but doesn't declare them as
    // build inputs, so regenerating them (`tauri icon`) leaves Cargo believing
    // nothing changed — the binary keeps the previous artwork until an
    // unrelated edit forces a rebuild. Watch the directory so a new icon
    // actually reaches the app.
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
