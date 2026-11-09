fn main() {
    // tauri-build embeds the app icon into the executable through a Windows
    // resource file. Cargo only re-runs a build script when one of its declared
    // inputs changes, and the icon is not tracked by default — so replacing
    // icons/icon.ico and rebuilding kept the *previous* bitmap inside the exe
    // (the old orange circle) until a forced clean rebuild. Declare the assets
    // explicitly so an icon swap always reaches the binary.
    println!("cargo:rerun-if-changed=icons");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    tauri_build::build()
}
