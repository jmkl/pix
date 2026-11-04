//! Folder listing and next/previous navigation.

use std::path::{Path, PathBuf};

const EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "bmp", "tif", "tiff"];

pub struct Navigator {
    files: Vec<PathBuf>,
    index: usize,
}

impl Navigator {
    /// Build a navigator from the opened file: scan its folder, filter by
    /// extension, natural-sort, and position on the opened file.
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let path = path.canonicalize()?;
        let dir = path.parent().unwrap_or_else(|| Path::new("."));

        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| is_image(p))
            .collect();
        files.sort_by(|a, b| {
            natord::compare_ignore_case(
                &a.file_name().unwrap_or_default().to_string_lossy(),
                &b.file_name().unwrap_or_default().to_string_lossy(),
            )
        });

        let index = files.iter().position(|p| *p == path).unwrap_or(0);
        Ok(Self { files, index })
    }

    pub fn current(&self) -> Option<&Path> {
        self.files.get(self.index).map(PathBuf::as_path)
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Move by `delta` with wraparound. Returns the new current path.
    pub fn step(&mut self, delta: isize) -> Option<&Path> {
        let n = self.files.len() as isize;
        if n == 0 {
            return None;
        }
        self.index = (self.index as isize + delta).rem_euclid(n) as usize;
        self.current()
    }

    /// Path at `delta` away from the current one, without moving (for preloading).
    pub fn peek(&self, delta: isize) -> Option<&Path> {
        let n = self.files.len() as isize;
        if n == 0 {
            return None;
        }
        let i = (self.index as isize + delta).rem_euclid(n) as usize;
        self.files.get(i).map(PathBuf::as_path)
    }
}

fn is_image(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}
