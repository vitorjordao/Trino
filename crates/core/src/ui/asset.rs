//! Baked screens and string tables kept parsed across frames, re-parsed
//! when the platform hands out new bytes (the PC's live reload).

use crate::platform::{BlobId, Platform};

use super::{StringTable, UiError, UiScreen};

/// A screen baked into a blob (`ui/<name>`). Fetch it in `update_with`
/// (where the platform is); draw with [`screen`](Self::screen).
pub struct ScreenBlob {
    id: BlobId,
    bytes: Option<&'static [u8]>,
    screen: Option<UiScreen<'static>>,
    /// Why the last bytes did not parse.
    pub error: Option<UiError>,
}

impl ScreenBlob {
    /// A blob by id (`BlobId::from_path("ui/pause")`).
    pub const fn from_id(id: BlobId) -> ScreenBlob {
        ScreenBlob {
            id,
            bytes: None,
            screen: None,
            error: None,
        }
    }

    /// The blob of `ui/<name>`: `ScreenBlob::new("pause")` reads `ui/pause`.
    pub fn new(name: &str) -> ScreenBlob {
        let mut path = crate::text::TextBuf::<64>::new();
        let _ = core::fmt::Write::write_fmt(&mut path, format_args!("ui/{name}"));
        ScreenBlob::from_id(BlobId::from_path(path.as_str()))
    }

    /// The screen, re-parsed when the blob changed; `None` while the blob
    /// is missing or broken ([`error`](Self::error)).
    pub fn get(&mut self, platform: &dyn Platform) -> Option<&UiScreen<'static>> {
        let bytes = platform.blob(self.id);
        if bytes.map(<[u8]>::as_ptr) != self.bytes.map(<[u8]>::as_ptr) {
            self.bytes = bytes;
            self.screen = None;
            self.error = None;
            if let Some(b) = bytes {
                match UiScreen::parse(b) {
                    Ok(s) => self.screen = Some(s),
                    Err(e) => self.error = Some(e),
                }
            }
        }
        self.screen.as_ref()
    }

    /// The screen as last fetched.
    pub fn screen(&self) -> Option<&UiScreen<'static>> {
        self.screen.as_ref()
    }
}

/// A language's string table (`i18n/<lang>`), like [`ScreenBlob`].
#[derive(Clone, Copy, Debug)]
pub struct StringsBlob {
    id: BlobId,
    bytes: Option<&'static [u8]>,
    table: Option<StringTable<'static>>,
}

impl StringsBlob {
    /// The blob of `i18n/<lang>`.
    pub fn new(lang: &str) -> StringsBlob {
        let mut path = crate::text::TextBuf::<64>::new();
        let _ = core::fmt::Write::write_fmt(&mut path, format_args!("i18n/{lang}"));
        StringsBlob {
            id: BlobId::from_path(path.as_str()),
            bytes: None,
            table: None,
        }
    }

    pub fn get(&mut self, platform: &dyn Platform) -> Option<&StringTable<'static>> {
        let bytes = platform.blob(self.id);
        if bytes.map(<[u8]>::as_ptr) != self.bytes.map(<[u8]>::as_ptr) {
            self.bytes = bytes;
            self.table = bytes.and_then(|b| StringTable::parse(b).ok());
        }
        self.table.as_ref()
    }

    pub fn table(&self) -> Option<&StringTable<'static>> {
        self.table.as_ref()
    }
}
