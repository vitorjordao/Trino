//! Compiles the 3DS shim's `T3AU` header check for the host (issue #121):
//! `c/t3au_check.c` over `crates/platform-3ds/shim/t3au_header.h`, the file
//! the shim itself includes. Under `cargo fuzz` (AddressSanitizer on) the C
//! is instrumented too: ASan always, libFuzzer's coverage when the compiler
//! is clang (`-fsanitize=fuzzer-no-link`; gcc has no equivalent, so there
//! the Rust side's branches steer the fuzzer).

fn main() {
    println!("cargo:rerun-if-changed=c/t3au_check.c");
    println!("cargo:rerun-if-changed=../platform-3ds/shim/t3au_header.h");
    println!("cargo:rerun-if-env-changed=CC");
    if std::env::var_os("CARGO_FEATURE_HOST").is_none() {
        return;
    }
    let mut build = cc::Build::new();
    build.file("c/t3au_check.c").warnings(true);
    let sanitize = std::env::var("CARGO_CFG_SANITIZE").unwrap_or_default();
    if sanitize.split(',').any(|s| s == "address") {
        let clang = build.get_compiler().is_like_clang();
        build.flag("-fsanitize=address");
        if clang {
            build.flag("-fsanitize=fuzzer-no-link");
        }
    }
    build.compile("trino_fuzz_t3au");
}
