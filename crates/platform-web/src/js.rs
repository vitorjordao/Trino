//! Bindings to `glue.js` (the KTX2 transcoder, the page's log and the test
//! harness's state).

use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/src/glue.js")]
extern "C" {
    /// Load Binomial's transcoder; `false` when its files are missing.
    pub async fn trino_init_basis(js_url: &str, wasm_url: &str) -> JsValue;
    pub fn trino_basis_ready() -> bool;
    /// `[width, height, hasAlpha, Uint8Array]`.
    #[wasm_bindgen(catch)]
    pub fn trino_transcode_ktx2(bytes: &[u8], format: u32) -> Result<js_sys::Array, JsValue>;
    /// `level`: 0 info, 1 warning, 2 error.
    pub fn trino_log(line: &str, level: u32);
    pub fn trino_finish(pass: bool, error: Option<String>);
    pub fn trino_info(key: &str, value: &str);
    pub fn trino_frame(name: &str, width: u32, height: u32, rgba: &[u8]);
    pub fn trino_audio_banner(show: bool);
    /// The canvas's unsafe borders, CSS pixels `[left, top, right, bottom]`.
    pub fn trino_safe_area(canvas: &web_sys::HtmlCanvasElement) -> js_sys::Array;
    pub fn trino_error_overlay(text: &str);
}

/// A log line: errors for failures and panics, warnings for warnings.
pub fn log(line: &str) {
    let trimmed = line.trim_end();
    let level = if trimmed.starts_with("TRINO_TEST_FAIL") || trimmed.contains("panicked") {
        2
    } else if trimmed.starts_with("warning") {
        1
    } else {
        0
    };
    for l in trimmed.lines() {
        trino_log(l, level);
    }
}
