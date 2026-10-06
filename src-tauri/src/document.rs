//! The session as app-kit's document (epic 32, story 4): Edit > Undo and Redo,
//! File > New, Open, Save and Revert, the unsaved mark and the close guard all
//! run against it.
//!
//! Implemented on `Audio`, the state Tauri manages, because app-kit finds the
//! document with `app.state::<H>()`. Everything here goes through the session;
//! the controller map follows afterwards, with the session's lock released.
//!
//! The webview polls values and meters but not materials, modulation or the
//! tracker, so whatever replaces the document under it says so:
//! `HISTORY_STEPPED` after an undo or redo, `DOCUMENT_OPENED` (with the load
//! report) after open and new.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use preset_app_kit::{Document, History};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::session::LoadReport;
use crate::{follow_map, midi, Audio};

/// An undo or redo changed the document; the payload is what it undid or redid.
pub const HISTORY_STEPPED: &str = "history-stepped";
/// The document was replaced by a file or a new one; the payload is a
/// `LoadReport`.
pub const DOCUMENT_OPENED: &str = "document-opened";

impl Audio {
    /// The controller map the document names, now that it changed. Answers a
    /// map this Mac lacks.
    fn follow<R: Runtime>(&self, app: &AppHandle<R>) -> Option<String> {
        let ctl = app.state::<Arc<midi::Controllers>>();
        follow_map(self, &ctl)
    }

    fn step<R: Runtime>(&self, app: &AppHandle<R>, label: Option<String>) {
        if let Some(label) = label {
            self.follow(app);
            let _ = app.emit(HISTORY_STEPPED, label);
        }
    }

    fn replaced<R: Runtime>(&self, app: &AppHandle<R>, mut report: LoadReport) {
        report.map_missing = self.follow(app);
        let _ = app.emit(DOCUMENT_OPENED, report);
    }
}

impl History for Audio {
    fn undo_label(&self) -> Option<String> {
        self.session.history().0
    }

    fn redo_label(&self) -> Option<String> {
        self.session.history().1
    }

    fn undo_labels(&self) -> Vec<String> {
        self.session.history_labels().0
    }

    fn redo_labels(&self) -> Vec<String> {
        self.session.history_labels().1
    }

    fn undo<R: Runtime>(&self, app: &AppHandle<R>) -> Result<(), String> {
        let label = self.session.undo()?;
        self.step(app, label);
        Ok(())
    }

    fn redo<R: Runtime>(&self, app: &AppHandle<R>) -> Result<(), String> {
        let label = self.session.redo()?;
        self.step(app, label);
        Ok(())
    }
}

impl Document for Audio {
    fn is_unsaved(&self) -> bool {
        self.session.is_unsaved()
    }

    fn path(&self) -> Option<PathBuf> {
        self.session.path()
    }

    fn untitled_number(&self) -> u32 {
        self.session.untitled_number()
    }

    fn save<R: Runtime>(&self, _app: &AppHandle<R>, path: &Path) -> Result<(), String> {
        self.session.save(path)
    }

    fn open<R: Runtime>(&self, app: &AppHandle<R>, path: &Path) -> Result<(), String> {
        let report = self.session.open(path)?;
        self.replaced(app, report);
        Ok(())
    }

    fn new_document<R: Runtime>(&self, app: &AppHandle<R>, untitled: u32) -> Result<(), String> {
        self.session.new_document(untitled)?;
        self.replaced(app, LoadReport::default());
        Ok(())
    }
}
