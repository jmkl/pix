//! Copy an RGBA image to the system clipboard without blocking the UI thread.

use std::borrow::Cow;
use std::sync::Arc;
use std::thread;

pub fn copy_image(width: u32, height: u32, rgba: Arc<Vec<u8>>) {
    thread::spawn(move || {
        let result = arboard::Clipboard::new().and_then(|mut cb| {
            cb.set_image(arboard::ImageData {
                width: width as usize,
                height: height as usize,
                bytes: Cow::Borrowed(rgba.as_slice()),
            })
        });
        if let Err(e) = result {
            log::warn!("clipboard copy failed: {e}");
        }
    });
}
