//! OS-level services a game may need, abstracted per platform.

pub trait Platform {
    /// Monotonic time in microseconds since an arbitrary epoch.
    fn now_micros(&self) -> u64;

    /// Debug logging. Routed to stdout on PC, ISViewer on N64, 3dslink
    /// stdout on 3DS. Stripped from release console builds.
    fn log(&self, message: &str);
}
