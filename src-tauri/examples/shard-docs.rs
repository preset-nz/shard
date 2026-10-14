//! `just docs`: one markdown page per Shard node kind in `docs/nodes/`, from the
//! words the kinds carry (`object_model::kinds`). `--check` writes nothing and
//! fails on a missing, stale or orphaned page or a kind without a summary
//! (`just docs-check`, part of `just check`).
//!
//! The palette's effects are one kind each, never one per instance: an
//! instance is a node of its kind, and the kind's rows are the page.

use std::path::PathBuf;
use std::process::ExitCode;

use rhizome_pom::{Document, MemoryStore};
use shard_lib::object_model::Shard;

fn main() -> ExitCode {
    let check = std::env::args().any(|a| a == "--check");
    let doc = Document::<Shard>::new(MemoryStore::default()).expect("Shard's model builds");
    let mut schema = doc.tree().registry().schema();
    // rhizome-pom's own `preset` node is bookkeeping for saved presets, not
    // something a person adds; it carries no words and gets no page.
    schema.types.retain(|t| t.name != "preset");
    let schema = preset_docs::Catalogue::from(&schema);

    let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/nodes"));
    if !check {
        match preset_docs::write(&dir, &schema, "Shard") {
            Ok(files) => {
                for f in files {
                    println!("wrote {f}");
                }
            }
            Err(e) => {
                eprintln!("shard-docs: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    let findings = match preset_docs::check(&dir, &schema, "Shard") {
        Ok(f) => f,
        Err(e) => {
            eprintln!("shard-docs: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut failed = false;
    for f in &findings {
        // Without --check the pages were just written; what remains is for a person.
        let fails = check && !f.is_warning();
        failed |= fails;
        eprintln!("{}: {f}", if fails { "error" } else { "warning" });
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
