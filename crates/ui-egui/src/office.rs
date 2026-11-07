//! Host automation over the visible SlideApp. No second presentation/session is created.
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::{SlideApp, async_saves::SaveOutcome};
use serde_json::{Value, json};
pub mod drafts;

pub const MAX_BYTES: usize = 32 << 20;
pub const METHODS: &[&str] = &["office.open", "office.commands", "office.execute", "office.inspect", "office.export", "office.save", "office.close"];
const COMMANDS: &[&str] = &[
    "document.inspect",
    "slide.inspect",
    "slide.new",
    "slide.delete",
    "slide.duplicate",
    "slide.move",
    "slide.go",
    "slide.hide",
    "slide.rename",
    "slide.layout",
    "shape.insert",
    "insert.textBox",
    "shape.inspect",
    "shape.move",
    "shape.resize",
    "shape.setBounds",
    "shape.fill",
    "shape.line",
    "shape.rotate",
    "text.edit",
    "text.exit",
    "text.insert",
    "text.set",
    "text.get",
    "text.select",
    "text.selectAll",
    "text.delete",
    "edit.undo",
    "edit.redo",
    "edit.pasteText",
    "edit.delete",
    "edit.duplicate",
    "edit.select",
    "edit.selectAll",
    "edit.deselect",
    "format.bold",
    "format.italic",
    "format.underline",
    "format.font",
    "format.size",
    "format.color",
    "arrange.align",
    "arrange.distribute",
    "arrange.group",
    "arrange.ungroup",
    "arrange.rotateLeft",
    "arrange.rotateRight",
];
pub fn command_allowed(command: &str) -> bool {
    COMMANDS.contains(&command)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    active: Option<u64>,
    documents: Vec<(u64, u64, usize)>,
    drafts: String,
}
#[derive(Default)]
pub(crate) struct OfficeState {
    key: Option<Key>,
    revision: u64,
    pending: BTreeMap<u64, Pending>,
    close_after_save: BTreeMap<u64, DeferredClose>,
    pub(crate) drafts: drafts::Drafts,
}
struct DeferredClose {
    ticket: u64,
    engine_revision: u64,
    acknowledged: bool,
}
struct Pending {
    document: u64,
    engine_revision: u64,
    name: String,
    copy: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OfficeError {
    pub code: &'static str,
    pub message: &'static str,
}
pub type OfficeResult<T> = Result<T, OfficeError>;
pub enum OfficeReply {
    Json(Value),
    Bytes { metadata: Value, bytes: Vec<u8> },
    Pending,
}
fn error(code: &'static str, message: &'static str) -> OfficeError {
    OfficeError { code, message }
}

impl SlideApp {
    /// Uses actual document identities/revisions, never an automation-only edit counter.
    /// Selection revisions are conservatively included. Human edits and undo/redo all use
    /// Session's revision source; the Arc identity additionally detects copy-on-write changes.
    pub fn office_revision(&mut self) -> u64 {
        self.office_bind_dialog();
        let key = Key {
            active: self.session.active().map(|d| d.uid),
            documents: self.session.documents().iter().map(|d| (d.uid, d.revision, Arc::as_ptr(&d.doc) as usize)).collect(),
            drafts: format!(
                "{:?}:{:?}:{:?}",
                self.dialog.as_ref().filter(|d| d.document_draft()).map(|d| (&d.id, &d.owner, &d.fields)),
                self.office.drafts.text.iter().map(|(k, d)| (k, &d.owner, &d.text)).collect::<Vec<_>>(),
                self.notes_buf.2.edited.then_some((&self.notes_buf.0, &self.notes_buf.1, &self.notes_buf.2.body))
            ),
        };
        if let Some(old) = &self.office.key {
            if old != &key {
                let mut delta = 0u64;
                for (uid, rev, ptr) in &key.documents {
                    delta = delta.saturating_add(match old.documents.iter().find(|d| d.0 == *uid) {
                        Some((_, before, previous)) => rev.saturating_sub(*before).max(u64::from(previous != ptr)),
                        None => 1,
                    });
                }
                delta = delta.saturating_add(old.documents.iter().filter(|d| !key.documents.iter().any(|k| k.0 == d.0)).count() as u64);
                self.office.revision = self.office.revision.saturating_add(delta.max(1)).min(9_007_199_254_740_991);
            }
        }
        self.office.key = Some(key);
        self.office.revision
    }
    pub fn office_has_unsaved_documents(&self) -> bool {
        self.office_has_drafts()
            || !self.office.pending.is_empty()
            || !self.pending_files.is_empty()
            || self.opening.is_some()
            || self.session.documents().iter().any(|d| d.is_dirty())
    }
    pub(crate) fn office_pending_count(&self) -> usize {
        self.office.pending.len()
    }
    pub fn office_has_pending_saves(&self) -> bool {
        !self.office.pending.is_empty()
    }
    pub(crate) fn office_document_pending(&self, uid: u64) -> bool {
        self.office.pending.values().any(|s| s.document == uid)
    }

    pub(crate) fn office_track_save(&mut self, ticket: u64, uid: u64, engine_revision: u64, name: String) -> Result<(), String> {
        self.office_track_write(ticket, uid, engine_revision, name, false)
    }
    pub(crate) fn office_track_copy(&mut self, ticket: u64, uid: u64, engine_revision: u64, name: String) -> Result<(), String> {
        self.office_track_write(ticket, uid, engine_revision, name, true)
    }
    fn office_track_write(&mut self, ticket: u64, uid: u64, engine_revision: u64, name: String, copy: bool) -> Result<(), String> {
        if self.office.pending.len() >= 8 || self.office.pending.contains_key(&ticket) {
            return Err("host save capacity exceeded".into());
        }
        let _ = self.services.async_saves.as_ref().and_then(|s| s.take_started());
        if !copy {
            self.session.host_save_started(uid);
        }
        self.office.pending.insert(ticket, Pending { document: uid, engine_revision, name, copy });
        Ok(())
    }
    /// The human Save Changes action owns this specific presentation and save ticket.
    /// It remains deferred while another write is pending, including a second save of the
    /// same revision. Later edits or any failed owned write cancel automatic close.
    pub fn office_save_and_close(&mut self) -> Result<(), String> {
        let d = self.session.active().ok_or("no presentation is open")?;
        let (uid, engine_revision) = (d.uid, d.revision);
        self.save_to_host(None)?;
        let ticket = self
            .office
            .pending
            .iter()
            .rev()
            .find(|(_, save)| save.document == uid && !save.copy && save.engine_revision == engine_revision)
            .map(|(ticket, _)| *ticket)
            .ok_or("host save did not create a ticket")?;
        self.office.close_after_save.insert(uid, DeferredClose { ticket, engine_revision, acknowledged: false });
        Ok(())
    }
    /// Called before publishing save responses. Acknowledgement can affect only its original
    /// owner and exact engine revision; active tab changes never retarget a save.
    pub fn office_poll_saves(&mut self) {
        let Some(saves) = self.services.async_saves.clone() else { return };
        for (ticket, outcome) in saves.take_finished() {
            let Some(save) = self.office.pending.remove(&ticket) else { continue };
            if !matches!(outcome, SaveOutcome::Saved) {
                self.office.close_after_save.remove(&save.document);
            } else if let Some(action) = self.office.close_after_save.get_mut(&save.document) {
                if action.ticket == ticket {
                    action.acknowledged = true;
                }
            }
            if save.copy {
                // A copy's acknowledgement never changes the source's path/saved snapshot.
                self.set_status(match outcome {
                    SaveOutcome::Saved => "Copy exported by host",
                    SaveOutcome::Cancelled | SaveOutcome::Failed(_) => "Copy export failed or was cancelled; source presentation was preserved",
                });
                continue;
            }
            match outcome {
                SaveOutcome::Saved => {
                    if self.session.acknowledge_host_save(save.document, save.engine_revision, save.name) {
                        self.set_status("Saved by host");
                    } else {
                        self.set_status("Snapshot saved; newer changes remain unsaved");
                    }
                }
                SaveOutcome::Cancelled | SaveOutcome::Failed(_) => {
                    self.session.host_save_started(save.document);
                    self.set_status("Host save failed or was cancelled; changes remain unsaved");
                }
            }
        }
        let owners: Vec<u64> = self.office.close_after_save.keys().copied().collect();
        for uid in owners {
            let Some(action) = self.office.close_after_save.get(&uid) else { continue };
            let Some(owner) = self.session.documents().iter().find(|d| d.uid == uid) else {
                self.office.close_after_save.remove(&uid);
                continue;
            };
            if owner.revision != action.engine_revision {
                self.office.close_after_save.remove(&uid);
                continue;
            }
            if self.office_has_drafts()
                || !action.acknowledged
                || self.office_document_pending(uid)
                || !self.pending_files.is_empty()
                || self.opening.is_some()
                || owner.is_dirty()
            {
                continue;
            }
            let active = self.session.active().map(|d| d.uid);
            if let Some(index) = self.session.documents().iter().position(|d| d.uid == uid) {
                self.session.close_document(index);
            }
            if let Some(index) = self.session.documents().iter().position(|d| Some(d.uid) == active) {
                self.session.set_active(index);
            }
            self.office.close_after_save.remove(&uid);
        }
    }
    pub fn office_call(&mut self, method: &str, params: &Value, bytes: Option<&[u8]>) -> OfficeResult<OfficeReply> {
        self.office_poll_saves();
        let revision = self.office_revision();
        if let Some(expected) = params.get("expectedRevision") {
            if expected.as_u64() != Some(revision) {
                return Err(error("STALE_REVISION", "Presentation revision does not match"));
            }
        }
        if !METHODS.contains(&method) {
            return Err(error("UNSUPPORTED", "Method is not implemented"));
        }
        if self.office_has_drafts() && matches!(method, "office.open" | "office.close" | "office.export" | "office.save" | "office.execute") {
            let query = method == "office.execute"
                && params
                    .get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|id| matches!(id, "document.inspect" | "slide.inspect" | "shape.inspect" | "text.get"));
            if !query {
                return Err(error("UNCOMMITTED_DRAFT", "Post or explicitly discard owned document drafts first"));
            }
        }
        if revision == 9_007_199_254_740_991 && matches!(method, "office.open" | "office.execute" | "office.close") {
            return Err(error("REVISION_LIMIT", "Frame revision lifetime is exhausted"));
        }
        let result = match method {
            "office.open" => {
                if self.office_has_unsaved_documents() || self.opening.is_some() {
                    return Err(error("UNSAVED_CHANGES", "Save all owned presentations and wait for pending work before opening"));
                }
                let bytes = bytes.ok_or(error("INVALID_ARGUMENT", "Open requires byte data"))?;
                if bytes.len() > MAX_BYTES {
                    return Err(error("LIMIT", "Open exceeds the 32 MiB bound"));
                }
                let name = params.get("name").and_then(Value::as_str).unwrap_or("Presentation.pptx");
                if name.len() > 255 || name.contains(['/', '\\', '\0']) {
                    return Err(error("INVALID_ARGUMENT", "Open requires a bare bounded file name"));
                }
                let before = self.session.active().map(|d| d.uid);
                self.open_from_host(Some(name), bytes);
                let owner = self
                    .session
                    .active()
                    .map(|d| d.uid)
                    .filter(|uid| Some(*uid) != before)
                    .ok_or(error("OPEN_FAILED", "Presentation could not be opened"))?;
                // Replacement is committed only after parsing succeeds, and only clean owners
                // can reach this point. No hidden dirty presentation is discarded.
                for index in (0..self.session.documents().len()).rev() {
                    if self.session.documents().get(index).is_some_and(|d| d.uid != owner) {
                        self.session.close_document(index);
                    }
                }
                self.office_revision();
                json!({"confirmed":true,"app":"deckcraft","revision":self.office.revision,"documentId":owner.to_string()})
            }
            "office.commands" => {
                let query = params.get("query").and_then(Value::as_str).unwrap_or("");
                if query.len() > 512 {
                    return Err(error("LIMIT", "Command query is too long"));
                }
                let query = query.to_ascii_lowercase();
                let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(128).clamp(1, 256) as usize;
                let commands: Vec<_> = self
                    .session
                    .commands()
                    .into_iter()
                    .filter(|c| {
                        command_allowed(c.id)
                            && (query.is_empty() || c.id.to_ascii_lowercase().contains(&query) || c.label.to_ascii_lowercase().contains(&query))
                    })
                    .take(limit)
                    .collect();
                serde_json::to_value(commands).map_err(|_| error("ENGINE_ERROR", "Cannot encode command registry"))?
            }
            "office.inspect" => {
                let value =
                    self.session.execute("document.inspect", &json!({})).map_err(|_| error("ENGINE_ERROR", "Presentation inspection failed"))?;
                json!({"document":value,"dirty":self.office_has_unsaved_documents(),"documentId":self.session.active().map(|d| d.uid.to_string()),"ownedDocuments":self.session.documents().len()})
            }
            "office.execute" => {
                let command = params.get("command").and_then(Value::as_str).ok_or(error("INVALID_ARGUMENT", "A command is required"))?;
                if !command_allowed(command) || !self.session.commands().iter().any(|c| c.id == command) {
                    return Err(error("UNSUPPORTED", "Command is outside the host document capability"));
                }
                let arguments = params.get("params").cloned().unwrap_or(json!({}));
                if !arguments.is_object() {
                    return Err(error("INVALID_ARGUMENT", "Command parameters must be an object"));
                }
                self.run(command, arguments).map_err(|_| error("ENGINE_ERROR", "Presentation command failed"))?
            }
            "office.export" => {
                let format = params.get("format").and_then(Value::as_str).unwrap_or("pptx");
                if !["pptx", "deckcraft", "potx", "ppsx"].contains(&format) {
                    return Err(error("UNSUPPORTED", "Export format is not implemented by this bridge"));
                }
                let doc = self.session.doc().map_err(|_| error("NO_DOCUMENT", "No presentation is open"))?;
                let bytes =
                    deckcraft_engine::cmd::file::save_bytes(&doc.doc, format).map_err(|_| error("ENGINE_ERROR", "Presentation export failed"))?;
                if bytes.len() > MAX_BYTES {
                    return Err(error("LIMIT", "Export exceeds the 32 MiB bound"));
                }
                return Ok(OfficeReply::Bytes { metadata: json!({"name":format!("Presentation.{format}"),"revision":revision}), bytes });
            }
            "office.save" => {
                if self.services.host_save_versioned.is_none() {
                    return Err(error("UNSUPPORTED", "Durable host save is unavailable"));
                }
                if self.office.pending.len() >= 8 {
                    return Err(error("LIMIT", "Too many pending host saves"));
                }
                self.save_to_host(if params.get("saveAs").and_then(Value::as_bool) == Some(true) { Some("pptx") } else { None })
                    .map_err(|_| error("SAVE_FAILED", "Host save could not be started"))?;
                return Ok(OfficeReply::Pending);
            }
            "office.close" => {
                if self.office_has_pending_saves() || !self.pending_files.is_empty() || self.opening.is_some() {
                    return Err(error("UNSAVED_CHANGES", "Pending owned work must settle before close"));
                }
                let active = self.session.active().map(|d| d.uid);
                if self.session.documents().iter().any(|d| Some(d.uid) != active && d.is_dirty()) {
                    return Err(error("UNSAVED_CHANGES", "Another owned presentation has unsaved changes"));
                }
                if self.session.active().is_some_and(|d| d.is_dirty()) && params.get("discardChanges").and_then(Value::as_bool) != Some(true) {
                    return Err(error("UNSAVED_CHANGES", "Presentation has unsaved changes"));
                }
                self.run("file.close", json!({})).map_err(|_| error("ENGINE_ERROR", "Presentation close failed"))?;
                json!({"closed":true,"remainingDocuments":self.session.documents().len()})
            }
            _ => return Err(error("UNSUPPORTED", "Method is not implemented")),
        };
        self.office_revision();
        Ok(OfficeReply::Json(result))
    }
}

#[cfg(test)]
mod tests;
