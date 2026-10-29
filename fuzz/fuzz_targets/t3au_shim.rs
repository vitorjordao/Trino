#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| trino_fuzz::run("t3au_shim", data));
