//! Documents macOS asks the app to open: a double-click in Finder, `open
//! some.shard`, a file dropped on the Dock icon.
//!
//! They arrive as `RunEvent::Opened`, both at a cold start and while the app
//! is running. At a cold start the event can come before the webview has
//! mounted, when nothing is listening yet, so the path is held here until the
//! frontend asks for it. Once it has asked, later paths are emitted straight
//! away. Deciding both under one lock is what stops a path falling between
//! the two.

use std::path::PathBuf;
use std::sync::Mutex;

/// The event the frontend listens for once it is ready.
pub const OPEN_DOCUMENT: &str = "open-document";

/// What to do with a path macOS handed over.
#[derive(Debug, PartialEq, Eq)]
pub enum Offer {
    /// The frontend is listening: emit it now.
    Emit(String),
    /// The frontend has not mounted: it will ask.
    Held,
}

#[derive(Default)]
struct Inner {
    ready: bool,
    pending: Option<String>,
}

/// The hand-off between `RunEvent::Opened` and the frontend.
#[derive(Default)]
pub struct Opened(Mutex<Inner>);

impl Opened {
    /// A path macOS opened. Held until the frontend is ready. A second path
    /// before then replaces the first, since only one document is open at a
    /// time and the newer request is the one the person meant.
    pub fn offer(&self, path: String) -> Offer {
        let mut inner = self.0.lock().expect("opened poisoned");
        if inner.ready {
            Offer::Emit(path)
        } else {
            inner.pending = Some(path);
            Offer::Held
        }
    }

    /// The frontend has mounted and is listening. Returns whatever arrived
    /// before it did, once.
    pub fn take(&self) -> Option<String> {
        let mut inner = self.0.lock().expect("opened poisoned");
        inner.ready = true;
        inner.pending.take()
    }
}

/// The last Shard document among the URLs macOS sent. Anything else (a
/// folder, a web link, another app's file) is not ours to open.
pub fn document_path<'a>(urls: impl IntoIterator<Item = &'a tauri::Url>) -> Option<String> {
    urls.into_iter()
        .filter_map(|u| u.to_file_path().ok())
        .filter(|p: &PathBuf| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("shard"))
        })
        .last()
        .map(|p| p.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_before_the_frontend_is_held_and_delivered_once() {
        let o = Opened::default();
        assert_eq!(o.offer("/a.shard".into()), Offer::Held);
        assert_eq!(o.take().as_deref(), Some("/a.shard"));
        assert_eq!(o.take(), None);
    }

    #[test]
    fn a_path_after_the_frontend_is_emitted_not_held() {
        let o = Opened::default();
        assert_eq!(o.take(), None);
        assert_eq!(o.offer("/b.shard".into()), Offer::Emit("/b.shard".into()));
        assert_eq!(o.take(), None);
    }

    #[test]
    fn the_newest_path_before_the_frontend_wins() {
        let o = Opened::default();
        o.offer("/old.shard".into());
        o.offer("/new.shard".into());
        assert_eq!(o.take().as_deref(), Some("/new.shard"));
    }

    #[test]
    fn only_shard_files_are_documents() {
        let urls: Vec<tauri::Url> = [
            "file:///tmp/one.shard",
            "file:///tmp/two.SHARD",
            "file:///tmp/sound.wav",
            "https://example.com/x.shard",
        ]
        .iter()
        .map(|s| s.parse().unwrap())
        .collect();
        assert_eq!(document_path(&urls).as_deref(), Some("/tmp/two.SHARD"));
        assert_eq!(document_path(&urls[2..]), None);
    }
}
