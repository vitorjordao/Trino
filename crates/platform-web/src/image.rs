//! [`Platform::save_image`](trino_core::Platform::save_image) in a browser
//! (Bongo-Kart #183): the game's RGBA picture encoded as PNG and handed to the
//! player as a download (`<a download>` on a `blob:` URL), the only place a
//! page can put a file. The harness keeps it (`tools/web-test`).

use std::cell::RefCell;
use trino_core::{RgbaImage, SaveError};
use wasm_bindgen::JsCast;

thread_local! {
    /// The last download's object URL, revoked by the next one.
    static LAST_URL: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The image's PNG bytes (8-bit RGBA); `None` when its sizes disagree.
pub(crate) fn encode_png(image: &RgbaImage) -> Option<Vec<u8>> {
    if !image.is_valid() {
        return None;
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, image.width, image.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().ok()?;
        w.write_image_data(image.pixels).ok()?;
    }
    Some(out)
}

/// Download `bytes` as `file_name`. `Io` when the page can't (no document,
/// a blocked object URL).
pub(crate) fn download(file_name: &str, bytes: &[u8]) -> Result<(), SaveError> {
    let window = web_sys::window().ok_or(SaveError::Unsupported)?;
    let document = window.document().ok_or(SaveError::Unsupported)?;
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let props = web_sys::BlobPropertyBag::new();
    props.set_type("image/png");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &props)
        .map_err(|_| SaveError::Io)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(|_| SaveError::Io)?;
    let a = document
        .create_element("a")
        .ok()
        .and_then(|e| e.dyn_into::<web_sys::HtmlAnchorElement>().ok())
        .ok_or(SaveError::Io)?;
    a.set_href(&url);
    a.set_download(file_name);
    a.click();
    // Revoking at once can cancel the download in some browsers: the URL
    // lives until the next image.
    LAST_URL.with(|last| {
        if let Some(old) = last.replace(Some(url)) {
            let _ = web_sys::Url::revoke_object_url(&old);
        }
    });
    crate::js::trino_info("image", &format!("{file_name} {} bytes", bytes.len()));
    Ok(())
}
