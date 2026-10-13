//! Relink when the C shim changes. The shim object reaches the linker as a
//! `-Clink-arg` (see `xtask/src/n3ds.rs`), which cargo does not track:
//! without this, editing only the shim kept the old ELF.

fn main() {
    println!("cargo:rerun-if-env-changed=TRINO_3DS_SHIM");
    if let Ok(shim) = std::env::var("TRINO_3DS_SHIM") {
        println!("cargo:rerun-if-changed={shim}");
    }
}
