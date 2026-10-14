//! Relink when the engine's C shim changes: it reaches the linker as a
//! `-Clink-arg`, which cargo does not track.

fn main() {
    println!("cargo:rerun-if-env-changed=TRINO_3DS_SHIM");
    if let Ok(shim) = std::env::var("TRINO_3DS_SHIM") {
        println!("cargo:rerun-if-changed={shim}");
    }
}
