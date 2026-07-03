//! Hybrid source resolution: platform override beats shared master; nothing
//! found is an error, never a fallback.

use std::path::{Path, PathBuf};

use crate::manifest::Platform;

pub fn resolve_source(
    assets_root: &Path,
    platform: Platform,
    file: &str,
) -> Result<PathBuf, String> {
    let override_path = assets_root.join(platform.key()).join(file);
    if override_path.is_file() {
        return Ok(override_path);
    }
    let shared = assets_root.join("shared").join(file);
    if shared.is_file() {
        return Ok(shared);
    }
    Err(format!(
        "asset source `{file}` not found: looked at {} and {}",
        override_path.display(),
        shared.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_wins_over_shared() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("shared/sprites")).unwrap();
        std::fs::create_dir_all(root.join("n64/sprites")).unwrap();
        std::fs::write(root.join("shared/sprites/p.png"), b"shared").unwrap();
        std::fs::write(root.join("n64/sprites/p.png"), b"override").unwrap();

        let n64 = resolve_source(root, Platform::N64, "sprites/p.png").unwrap();
        assert!(n64.ends_with(Path::new("n64/sprites/p.png")));
        // Other platforms still get the shared master.
        let pc = resolve_source(root, Platform::Pc, "sprites/p.png").unwrap();
        assert!(pc.ends_with(Path::new("shared/sprites/p.png")));
    }

    #[test]
    fn missing_everywhere_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve_source(dir.path(), Platform::Pc, "nope.png").unwrap_err();
        assert!(err.contains("not found"));
    }
}
