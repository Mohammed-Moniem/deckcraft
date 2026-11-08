//! Uncommitted UI work has an owner even before it enters the engine's undo history.
use crate::SlideApp;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftOwner {
    pub uid: u64,
    pub revision: u64,
    model: usize,
    selection: String,
    slide: Option<u32>,
}
impl DraftOwner {
    pub fn of(app: &SlideApp) -> Option<Self> {
        app.session.active().map(|d| Self {
            uid: d.uid,
            revision: d.revision,
            model: Arc::as_ptr(&d.doc) as usize,
            selection: serde_json::to_string(&d.selection).unwrap_or_default(),
            slide: d.current_slide().map(|s| s.id.0),
        })
    }
    pub fn matches(&self, app: &SlideApp) -> bool {
        Self::of(app).as_ref() == Some(self)
    }
    /// Explicit recovery returns to the same UID and stable slide/shape identities. It can
    /// never attach a draft to the currently active replacement presentation.
    pub fn resume(&self, app: &mut SlideApp) -> Result<Self, String> {
        let index = app
            .session
            .documents()
            .iter()
            .position(|d| d.uid == self.uid)
            .ok_or("Original presentation is closed; copy or discard the retained draft")?;
        let mut selection: deckcraft_engine::Selection = serde_json::from_str(&self.selection).map_err(|_| "Draft target is unavailable")?;
        if selection.target != deckcraft_engine::Target::Slides {
            return Err("Restore the original master/layout history or copy/discard the draft".into());
        }
        let doc = &app.session.documents()[index];
        let slide = doc
            .doc
            .slides
            .iter()
            .position(|s| Some(s.id.0) == self.slide)
            .ok_or("Original slide was removed; undo its deletion or copy/discard the draft")?;
        if selection.shapes.iter().any(|id| deckcraft_model::find_shape(&doc.doc.slides[slide].shapes, *id).is_none()) {
            return Err("Original shape was removed; restore it or copy/discard the draft".into());
        }
        selection.slide = slide;
        app.session.set_active(index);
        app.session.execute("slide.go", &serde_json::json!({"index":slide})).map_err(|_| "Cannot restore original draft target")?;
        if let Some(doc) = app.session.active_mut() {
            doc.selection = selection;
        }
        Self::of(app).ok_or("Draft owner is unavailable".into())
    }
    pub fn history(&self, app: &mut SlideApp, redo: bool) -> Result<(), String> {
        let index = app.session.documents().iter().position(|d| d.uid == self.uid).ok_or("Original presentation is closed")?;
        app.session.set_active(index);
        app.session
            .execute(if redo { "edit.redo" } else { "edit.undo" }, &serde_json::json!({}))
            .map(|_| ())
            .map_err(|_| "Original presentation has no matching history step".into())
    }
}
#[derive(Clone)]
pub(crate) struct TextDraft {
    pub owner: DraftOwner,
    pub text: String,
    comment: Option<(usize, String)>,
    section: Option<(usize, String)>,
}
#[derive(Default)]
pub(crate) struct Drafts {
    pub text: BTreeMap<String, TextDraft>,
    pub committing: bool,
    discarded: BTreeSet<String>,
}
pub const MAX_DRAFT_TEXT: usize = 64 << 10;
const MAX_DRAFTS: usize = 32;
impl SlideApp {
    pub(crate) fn office_embedded(&self) -> bool {
        self.services.host_save_versioned.is_some()
    }
    pub fn office_has_drafts(&self) -> bool {
        self.office_embedded()
            && (self.dialog.as_ref().is_some_and(|d| d.document_draft()) || !self.office.drafts.text.is_empty() || self.notes_buf.2.edited)
    }
    pub(crate) fn office_bind_dialog(&mut self) {
        let owner = DraftOwner::of(self);
        if let Some(dialog) = &mut self.dialog
            && dialog.document_draft()
            && dialog.owner.is_none()
        {
            dialog.owner = owner;
        }
    }
    /// Keep drafts in app-owned bounded storage, not an index-only egui temporary slot.
    /// A stale draft is retained verbatim until explicitly discarded; it is never reseeded
    /// onto another document/slide/history state.
    pub(crate) fn office_text_draft(&self, key: &str, current: &str) -> String {
        self.office.drafts.text.get(key).map(|d| d.text.clone()).unwrap_or_else(|| current.into())
    }
    pub(crate) fn office_record_draft(&mut self, key: &str, original: &str, text: &str) -> bool {
        if text == original {
            self.office.drafts.text.remove(key);
            return true;
        }
        if text.len() > MAX_DRAFT_TEXT
            || key.len() > 256
            || (!self.office.drafts.text.contains_key(key) && self.office.drafts.text.len() >= MAX_DRAFTS)
        {
            self.set_status("Draft limit reached; post or discard existing work first");
            return false;
        }
        if let Some(draft) = self.office.drafts.text.get_mut(key) {
            draft.text = text.into();
            return true;
        }
        let Some(owner) = DraftOwner::of(self) else { return false };
        let comment = if key.starts_with("reply:") {
            key.rsplit(':').next().and_then(|v| v.parse::<usize>().ok()).and_then(|index| {
                self.session
                    .active()
                    .and_then(|d| d.current_slide())
                    .and_then(|slide| slide.comments.get(index))
                    .and_then(|c| serde_json::to_string(c).ok())
                    .map(|value| (index, value))
            })
        } else {
            None
        };
        let section = if key.starts_with("section:") {
            key.rsplit(':').next().and_then(|v| v.parse::<usize>().ok()).and_then(|index| {
                self.session.active().and_then(|d| d.doc.sections.get(index)).and_then(|s| serde_json::to_string(s).ok()).map(|value| (index, value))
            })
        } else {
            None
        };
        self.office.drafts.text.insert(key.into(), TextDraft { owner, text: text.into(), comment, section });
        true
    }
    pub(crate) fn office_discard_draft(&mut self, key: &str) {
        self.office.drafts.text.remove(key);
        if self.office.drafts.discarded.len() >= 64 {
            self.office.drafts.discarded.pop_first();
        }
        self.office.drafts.discarded.insert(key.into());
    }
    pub(crate) fn office_take_discard(&mut self, key: &str) -> bool {
        self.office.drafts.discarded.remove(key)
    }
    pub(crate) fn office_resume_draft(&mut self, key: &str) -> Result<(), String> {
        let draft = self.office.drafts.text.get(key).cloned().ok_or("No retained draft")?;
        if let Some((index, before)) = &draft.comment {
            let current = self
                .session
                .documents()
                .iter()
                .find(|d| d.uid == draft.owner.uid)
                .and_then(|d| d.doc.slides.iter().find(|s| Some(s.id.0) == draft.owner.slide))
                .and_then(|s| s.comments.get(*index))
                .and_then(|c| serde_json::to_string(c).ok());
            if current.as_ref() != Some(before) {
                return Err("Original comment changed; restore its history or copy/discard the retained reply".into());
            }
        }
        if let Some((index, before)) = &draft.section {
            let current = self
                .session
                .documents()
                .iter()
                .find(|d| d.uid == draft.owner.uid)
                .and_then(|d| d.doc.sections.get(*index))
                .and_then(|s| serde_json::to_string(s).ok());
            if current.as_ref() != Some(before) {
                return Err("Original section changed; restore its history or copy/discard the retained name".into());
            }
        }
        if key.starts_with("notes:") || key.starts_with("notespage:") || key.starts_with("section:") {
            let original: deckcraft_engine::Selection = serde_json::from_str(&draft.owner.selection).map_err(|_| "Original target is unavailable")?;
            let original_still_at_index = self
                .session
                .documents()
                .iter()
                .find(|d| d.uid == draft.owner.uid)
                .and_then(|d| d.doc.slides.get(original.slide))
                .is_some_and(|s| Some(s.id.0) == draft.owner.slide);
            if !original_still_at_index {
                return Err("Draft target order changed; restore that history or copy/discard the draft".into());
            }
        }
        let owner = if draft.owner.matches(self) { draft.owner.clone() } else { draft.owner.resume(self)? };
        if let Some(current) = self.office.drafts.text.get_mut(key) {
            current.owner = owner.clone();
        }
        if key.starts_with("reply:") {
            self.ui.pane = Some("comments".into());
        }
        if key.starts_with("notes:") {
            self.ui.notes = true;
        }
        if key.starts_with("notespage:") {
            self.ui.view = crate::ViewMode::NotesPage;
        }
        if key.starts_with("outline-") {
            self.ui.view = crate::ViewMode::Outline;
        }
        if key.starts_with("section:")
            && let Some((index, _)) = draft.section
        {
            self.ui.view = crate::ViewMode::Sorter;
            self.sorter.resume_rename(owner.uid, index, draft.text.clone());
        }
        if key.starts_with("property:") {
            let mut backstage = crate::backstage::Backstage::new(crate::backstage::Page::Info, (self.session.documents().len(), Some(owner.uid)));
            for (name, peer) in &mut self.office.drafts.text {
                if name.starts_with("property:") && peer.owner.uid == owner.uid {
                    backstage.restore_property_draft(name.rsplit(':').next().unwrap_or(""), &peer.text);
                    if peer.owner == draft.owner {
                        peer.owner = owner.clone();
                    }
                }
            }
            self.backstage = Some(backstage);
        }
        Ok(())
    }
    pub(crate) fn office_commit_draft(&mut self, key: &str, command: &str, params: Value) -> Result<Value, String> {
        if !self.office_embedded() {
            let result = self.run(command, params);
            if result.is_ok() {
                self.office.drafts.text.remove(key);
            }
            return result;
        }
        let draft = self.office.drafts.text.get(key).cloned().ok_or("No pending draft")?;
        if self.office_embedded() && !draft.owner.matches(self) {
            return Err("Draft owner changed; return to its original presentation or explicitly discard the draft".into());
        }
        self.office.drafts.committing = true;
        let result = self.run(command, params);
        self.office.drafts.committing = false;
        if result.is_ok() {
            self.office.drafts.text.remove(key);
            // Moving between Info fields may start one property draft before committing
            // another in the same UI pass. A property-only commit safely advances its peers.
            if command == "file.properties"
                && key.starts_with("property:")
                && let Some(owner) = DraftOwner::of(self)
            {
                for (key, peer) in &mut self.office.drafts.text {
                    if key.starts_with("property:") && peer.owner == draft.owner {
                        peer.owner = owner.clone();
                    }
                }
            }
        }
        result
    }
    pub(crate) fn office_draft_bar(&mut self, ui: &mut egui::Ui) {
        if !self.office_embedded() || self.office.drafts.text.is_empty() {
            return;
        }
        let drafts: Vec<_> = self.office.drafts.text.iter().map(|(key, draft)| (key.clone(), draft.clone())).collect();
        egui::Panel::top("owned-document-drafts").show(ui, |ui| {
            ui.label("Uncommitted document work: finish its editor or explicitly discard it before changing files.");
            for (key, draft) in drafts {
                ui.horizontal(|ui| {
                    let label = match key.split(':').next().unwrap_or("") {
                        "reply" => "Reply",
                        "notes" => "Speaker notes",
                        "notespage" => "Notes page",
                        "property" => "Document property",
                        "outline-title" => "Outline title",
                        "outline-body" => "Outline text",
                        "section" => "Section name",
                        _ => "Document text",
                    };
                    ui.label(format!("Uncommitted {label}"));
                    if !draft.owner.matches(self) {
                        ui.label("Original owner or history differs; posting is blocked.");
                        if ui.button("Return to draft owner").clicked()
                            && let Some(i) = self.session.documents().iter().position(|d| d.uid == draft.owner.uid)
                        {
                            self.session.set_active(i);
                        }
                        if ui.button("Resume on original target").clicked()
                            && let Err(error) = self.office_resume_draft(&key)
                        {
                            self.set_status(error);
                        }
                        if ui.button("Undo in original presentation").clicked()
                            && let Err(error) = draft.owner.history(self, false)
                        {
                            self.set_status(error);
                        }
                        if ui.button("Redo in original presentation").clicked()
                            && let Err(error) = draft.owner.history(self, true)
                        {
                            self.set_status(error);
                        }
                    }
                    if draft.owner.matches(self)
                        && ui.button("Continue editing draft").clicked()
                        && let Err(error) = self.office_resume_draft(&key)
                    {
                        self.set_status(error);
                    }
                    if ui.button("Discard draft").clicked() {
                        self.office_discard_draft(&key);
                        if key.starts_with("notes:") {
                            self.notes_buf.2.edited = false;
                        }
                        if key.starts_with("section:") {
                            self.sorter.renaming = None;
                        }
                    }
                });
                // Retain a visible, selectable recovery copy even when its original pane/window
                // is parked or its engine history changed. Display cannot commit to a new owner.
                if !draft.owner.matches(self) {
                    ui.add(egui::Label::new(draft.text).selectable(true));
                }
            }
        });
    }
}
