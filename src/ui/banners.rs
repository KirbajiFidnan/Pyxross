//! Discovery and nearest-neighbor loading for top-bar banner pixel-art assets.
//!
//! Scans `banners/` directories in priority order:
//! 1. `<cwd>/banners`
//! 2. `<exe_dir>/banners`
//! 3. `<config>/pyxross/banners`

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// One discovered banner image inside a `banners/` directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BannerEntry {
    pub name: String,
    pub path: PathBuf,
}

fn is_supported_image(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "bmp"
    )
}

/// Candidate `banners/` directories, highest priority first.
pub fn banner_search_dirs() -> Vec<PathBuf> {
    let mut dirs_out = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        dirs_out.push(cwd.join("banners"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let candidate = parent.join("banners");
            if !dirs_out.contains(&candidate) {
                dirs_out.push(candidate);
            }
        }
    }
    if let Some(cfg) = dirs::config_dir() {
        let candidate = cfg.join("pyxross").join("banners");
        if !dirs_out.contains(&candidate) {
            dirs_out.push(candidate);
        }
    }
    dirs_out
}

/// Scans the given directories for banner images, deduplicating by file name.
pub fn scan_banners_in(dirs: &[PathBuf]) -> Vec<BannerEntry> {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for dir in dirs {
        let Ok(read_dir) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = read_dir
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file() && is_supported_image(p))
            .collect();
        paths.sort();
        for path in paths {
            let Some(name) = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            if seen.insert(name.clone()) {
                entries.push(BannerEntry { name, path });
            }
        }
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

/// Scans all default `banners/` directories.
pub fn scan_banners() -> Vec<BannerEntry> {
    scan_banners_in(&banner_search_dirs())
}

/// Resolves a stored banner name or path to a [`BannerEntry`].
pub fn find_banner(spec: &str) -> Option<BannerEntry> {
    let path = Path::new(spec);
    if path.is_file() && is_supported_image(path) {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(spec)
            .to_string();
        return Some(BannerEntry {
            name,
            path: path.to_path_buf(),
        });
    }
    scan_banners().into_iter().find(|entry| entry.name == spec)
}

/// Decodes an image file into an RGBA [`egui::ColorImage`] and returns `[w, h]`.
pub fn load_banner_image(path: &Path) -> Result<(egui::ColorImage, [u32; 2]), String> {
    let img = image::open(path).map_err(|e| format!("Failed to open banner: {e}"))?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let color_image =
        egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw());
    Ok((color_image, [w, h]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_banners_in_finds_sorted_images_and_ignores_non_images() {
        let dir = std::env::temp_dir().join(format!("pyx_banners_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("z_banner.png"), b"x").unwrap();
        std::fs::write(dir.join("a_banner.PNG"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();

        let entries = scan_banners_in(&[dir.clone()]);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "a_banner.PNG");
        assert_eq!(entries[1].name, "z_banner.png");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
