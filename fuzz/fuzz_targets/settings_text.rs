#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| trino_fuzz::run("settings_text", data));
