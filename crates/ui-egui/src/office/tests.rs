#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::*;
use crate::{Services, async_saves::AsyncSaves};
use deckcraft_engine::Session;
use egui_kittest::kittest::Queryable;
use std::{cell::RefCell, rc::Rc};
type Got = Rc<RefCell<Vec<(u64, u64, u64, u64, String, Vec<u8>, bool)>>>;
fn app() -> (SlideApp, AsyncSaves, Got) {
    let saves = AsyncSaves::default();
    let finished = saves.clone();
    let got: Got = Rc::default();
    let output = got.clone();
    let services = Services {
        async_saves: Some(saves.clone()),
        host_save_versioned: Some(Box::new(move |name, bytes, _, uid, revision, wire, copy| {
            let ticket = finished.start();
            output.borrow_mut().push((ticket, uid, revision, wire, name.into(), bytes.into(), copy));
            Ok(ticket)
        })),
        ..Default::default()
    };
    let mut app = SlideApp::new(Session::new(), services);
    app.start_for_host().unwrap();
    app.office_revision();
    (app, saves, got)
}
fn call(app: &mut SlideApp, method: &str, params: Value) -> OfficeReply {
    app.office_call(method, &params, None).unwrap()
}
fn edit(app: &mut SlideApp) {
    app.run("slide.new", json!({"title":"Deck host lifecycle marker"})).unwrap();
}
fn uid(app: &SlideApp) -> u64 {
    app.session.active().unwrap().uid
}
fn ticket(got: &Got) -> u64 {
    got.borrow().last().unwrap().0
}
fn pptx() -> Vec<u8> {
    let s = Session::with_new();
    deckcraft_engine::cmd::file::save_bytes(&s.doc().unwrap().doc, "pptx").unwrap()
}
fn json_reply(value: OfficeReply) -> Value {
    match value {
        OfficeReply::Json(value) => value,
        _ => panic!("expected JSON"),
    }
}

#[test]
fn human_edits_undo_redo_and_tab_identity_advance_real_revision() {
    let (mut a, _, _) = app();
    let first = a.office_revision();
    edit(&mut a);
    let changed = a.office_revision();
    assert!(changed > first);
    a.session.execute("edit.undo", &json!({})).unwrap();
    let undo = a.office_revision();
    assert!(undo > changed);
    a.session.execute("edit.redo", &json!({})).unwrap();
    let redo = a.office_revision();
    assert!(redo > undo);
    a.run("file.new", json!({})).unwrap();
    assert!(a.office_revision() > redo);
    let current = a.office_revision();
    a.session.set_active(0);
    assert!(a.office_revision() > current);
}
#[test]
fn stale_revision_fails_before_mutation() {
    let (mut a, _, _) = app();
    let old = a.office_revision();
    edit(&mut a);
    let n = a.session.doc().unwrap().doc.slides.len();
    let err = a.office_call("office.execute", &json!({"command":"slide.new","expectedRevision":old}), None).err().unwrap();
    assert_eq!(err.code, "STALE_REVISION");
    assert_eq!(a.session.doc().unwrap().doc.slides.len(), n);
}
#[test]
fn programmatic_save_waits_and_adopts_only_after_ack() {
    let (mut a, saves, got) = app();
    edit(&mut a);
    let revision = a.office_revision();
    assert!(matches!(call(&mut a, "office.save", json!({})), OfficeReply::Pending));
    assert!(a.office_has_unsaved_documents());
    assert!(a.session.doc().unwrap().is_dirty());
    assert!(a.session.doc().unwrap().path.is_none());
    assert_eq!(got.borrow()[0].3, revision);
    saves.finish(ticket(&got), SaveOutcome::Saved);
    a.office_poll_saves();
    assert!(!a.office_has_unsaved_documents());
    assert!(a.session.doc().unwrap().path.is_some());
    assert_eq!(a.office_revision(), revision);
}
#[test]
fn ui_save_uses_identical_ticket_and_preserves_snapshot_bytes() {
    let (mut a, saves, got) = app();
    edit(&mut a);
    let count = a.session.doc().unwrap().doc.slides.len();
    a.save();
    assert!(a.office_has_pending_saves());
    let bytes = &got.borrow()[0].5;
    let reopened = deckcraft_engine::cmd::file::loaded_file("saved.deckcraft", bytes).unwrap();
    assert_eq!(reopened.doc.slides.len(), count);
    saves.finish(ticket(&got), SaveOutcome::Saved);
    a.office_poll_saves();
    assert!(!a.session.doc().unwrap().is_dirty());
}
#[test]
fn failed_save_retains_dirty_and_retry_succeeds() {
    let (mut a, saves, got) = app();
    edit(&mut a);
    a.save();
    saves.finish(ticket(&got), SaveOutcome::Failed("denied".into()));
    a.office_poll_saves();
    assert!(a.session.doc().unwrap().is_dirty());
    assert!(a.session.doc().unwrap().path.is_none());
    a.save();
    saves.finish(ticket(&got), SaveOutcome::Saved);
    a.office_poll_saves();
    assert!(!a.office_has_unsaved_documents());
}
#[test]
fn unchanged_save_failure_is_still_unconfirmed() {
    let (mut a, saves, got) = app();
    assert!(!a.session.doc().unwrap().is_dirty());
    a.save();
    assert!(a.session.doc().unwrap().is_dirty());
    saves.finish(ticket(&got), SaveOutcome::Cancelled);
    a.office_poll_saves();
    assert!(a.office_has_unsaved_documents());
}
#[test]
fn old_ack_cannot_clear_newer_edits_or_adopt_path() {
    let (mut a, saves, got) = app();
    edit(&mut a);
    a.save();
    let old = ticket(&got);
    edit(&mut a);
    let n = a.session.doc().unwrap().doc.slides.len();
    saves.finish(old, SaveOutcome::Saved);
    a.office_poll_saves();
    assert!(a.session.doc().unwrap().is_dirty());
    assert!(a.session.doc().unwrap().path.is_none());
    assert_eq!(a.session.doc().unwrap().doc.slides.len(), n);
}
#[test]
fn undo_redo_to_old_content_does_not_revalidate_an_old_ack() {
    let (mut a, saves, got) = app();
    edit(&mut a);
    a.save();
    a.run("edit.undo", json!({})).unwrap();
    a.run("edit.redo", json!({})).unwrap();
    saves.finish(ticket(&got), SaveOutcome::Saved);
    a.office_poll_saves();
    assert!(a.office_has_unsaved_documents());
    assert!(a.session.doc().unwrap().path.is_none());
}
#[test]
fn parked_owner_ack_never_retargets_active_document() {
    let (mut a, saves, got) = app();
    edit(&mut a);
    let owner = uid(&a);
    a.save();
    a.run("file.new", json!({})).unwrap();
    let active = uid(&a);
    assert_ne!(owner, active);
    saves.finish(ticket(&got), SaveOutcome::Saved);
    a.office_poll_saves();
    assert_eq!(uid(&a), active);
    assert!(a.session.doc().unwrap().path.is_none());
    let old = a.session.documents().iter().find(|d| d.uid == owner).unwrap();
    assert!(!old.is_dirty());
    assert!(old.path.is_some());
}
#[test]
fn pending_save_blocks_even_forced_host_close_and_ui_close() {
    let (mut a, _, _) = app();
    a.save();
    assert_eq!(a.office_call("office.close", &json!({"discardChanges":true}), None).err().unwrap().code, "UNSAVED_CHANGES");
    assert!(a.run("file.close", json!({})).is_err());
    assert_eq!(a.session.documents().len(), 1);
}
#[test]
fn parked_dirty_owner_blocks_open_and_close_of_a_clean_active_doc() {
    let (mut a, _, _) = app();
    edit(&mut a);
    a.run("file.new", json!({})).unwrap();
    assert!(!a.session.doc().unwrap().is_dirty());
    assert_eq!(a.office_call("office.open", &json!({"name":"new.pptx"}), Some(&pptx())).err().unwrap().code, "UNSAVED_CHANGES");
    assert_eq!(a.office_call("office.close", &json!({"discardChanges":true}), None).err().unwrap().code, "UNSAVED_CHANGES");
    assert_eq!(a.session.documents().len(), 2);
}
#[test]
fn open_replaces_only_clean_owners_after_successful_parsing() {
    let (mut a, _, _) = app();
    let old = uid(&a);
    a.run("file.new", json!({})).unwrap();
    let result = a.office_call("office.open", &json!({"name":"actual.pptx"}), Some(&pptx())).unwrap();
    assert_eq!(json_reply(result)["confirmed"], true);
    assert_ne!(uid(&a), old);
    assert_eq!(a.session.documents().len(), 1);
    assert_eq!(a.session.doc().unwrap().path.as_deref(), Some("actual.pptx"));
    let current = uid(&a);
    assert!(a.office_call("office.open", &json!({"name":"bad.pptx"}), Some(b"broken")).is_err());
    assert_eq!(uid(&a), current);
}
#[test]
fn export_roundtrip_is_not_a_save_ack() {
    let (mut a, _, _) = app();
    edit(&mut a);
    let before = a.office_revision();
    let count = a.session.doc().unwrap().doc.slides.len();
    let OfficeReply::Bytes { bytes, .. } = call(&mut a, "office.export", json!({"format":"pptx"})) else { panic!("missing bytes") };
    assert_eq!(deckcraft_engine::cmd::file::loaded_file("export.pptx", &bytes).unwrap().doc.slides.len(), count);
    assert_eq!(a.office_revision(), before);
    assert!(a.office_has_unsaved_documents());
}
#[test]
fn blocked_capabilities_are_truthfully_absent() {
    let (mut a, _, _) = app();
    let commands = json_reply(call(&mut a, "office.commands", json!({})));
    assert!(commands.as_array().unwrap().iter().any(|v| v["id"] == "slide.new"));
    assert!(!commands.as_array().unwrap().iter().any(|v| v["id"].as_str().unwrap().starts_with("file.")));
    assert_eq!(a.office_call("office.render", &json!({}), None).err().unwrap().code, "UNSUPPORTED");
    assert_eq!(a.office_call("office.execute", &json!({"command":"file.new"}), None).err().unwrap().code, "UNSUPPORTED");
}
#[test]
fn duplicate_unknown_and_cancelled_tickets_cannot_clear_later_work() {
    let (mut a, saves, got) = app();
    edit(&mut a);
    a.save();
    let id = ticket(&got);
    saves.finish(id + 999, SaveOutcome::Saved);
    a.office_poll_saves();
    assert!(a.office_has_pending_saves());
    saves.finish(id, SaveOutcome::Cancelled);
    saves.finish(id, SaveOutcome::Saved);
    a.office_poll_saves();
    assert!(a.office_has_unsaved_documents());
    edit(&mut a);
    saves.finish(id, SaveOutcome::Saved);
    a.office_poll_saves();
    assert!(a.office_has_unsaved_documents());
}
#[test]
fn pending_capacity_is_bounded() {
    let (mut a, _, got) = app();
    for _ in 0..8 {
        call(&mut a, "office.save", json!({}));
    }
    assert_eq!(got.borrow().len(), 8);
    assert_eq!(a.office_call("office.save", &json!({}), None).err().unwrap().code, "LIMIT");
    assert_eq!(got.borrow().len(), 8);
}

#[test]
fn actual_textbox_commands_edit_visible_document_and_export_text() {
    let (mut a, _, _) = app();
    let revision = a.office_revision();
    call(&mut a, "office.execute", json!({"command":"insert.textBox","params":{"text":"Visible slide marker"},"expectedRevision":revision}));
    let after = a.office_revision();
    assert!(after > revision);
    let inspection = json_reply(call(&mut a, "office.execute", json!({"command":"slide.inspect","params":{}})));
    assert!(inspection.to_string().contains("Visible slide marker"));
    let OfficeReply::Bytes { bytes, .. } = call(&mut a, "office.export", json!({"format":"pptx"})) else { panic!("missing bytes") };
    let reopened = deckcraft_engine::cmd::file::loaded_file("actual.pptx", &bytes).unwrap();
    assert!(reopened.doc.slides.iter().flat_map(|s| &s.shapes).any(|s| s.text.as_ref().is_some_and(|t| t.text().contains("Visible slide marker"))));
    assert!(a.office_has_unsaved_documents());
}
#[test]
fn pending_file_read_owns_work_before_bytes_arrive() {
    let (mut a, _, _) = app();
    a.begin_external_file("drop");
    assert!(a.office_has_unsaved_documents());
    assert_eq!(a.office_call("office.open", &json!({"name":"next.pptx"}), Some(&pptx())).err().unwrap().code, "UNSAVED_CHANGES");
    assert_eq!(a.office_call("office.close", &json!({"discardChanges":true}), None).err().unwrap().code, "UNSAVED_CHANGES");
}
#[test]
fn callback_failure_never_marks_unchanged_owner_saved() {
    let (mut a, _, _) = app();
    a.services.host_save_versioned = Some(Box::new(|_, _, _, _, _, _, _| Err("storage unavailable".into())));
    assert_eq!(a.office_call("office.save", &json!({}), None).err().unwrap().code, "SAVE_FAILED");
    assert!(a.session.doc().unwrap().is_dirty());
    assert!(a.session.doc().unwrap().path.is_none());
    assert!(!a.office_has_pending_saves());
}
#[test]
fn two_snapshots_cannot_clear_newer_work_out_of_order() {
    let (mut a, saves, got) = app();
    edit(&mut a);
    a.save();
    let first = ticket(&got);
    edit(&mut a);
    a.save();
    let second = ticket(&got);
    saves.finish(second, SaveOutcome::Saved);
    a.office_poll_saves();
    assert!(!a.session.doc().unwrap().is_dirty());
    assert!(a.office_has_unsaved_documents());
    saves.finish(first, SaveOutcome::Saved);
    a.office_poll_saves();
    assert!(!a.office_has_unsaved_documents());
    edit(&mut a);
    a.save();
    let third = ticket(&got);
    a.save();
    let fourth = ticket(&got);
    saves.finish(third, SaveOutcome::Saved);
    a.office_poll_saves();
    saves.finish(fourth, SaveOutcome::Failed("denied".into()));
    a.office_poll_saves();
    assert!(a.office_has_unsaved_documents());
}
#[test]
fn unknown_commands_formats_and_file_names_are_rejected_without_effects() {
    let (mut a, _, _) = app();
    let before = a.office_revision();
    for command in ["shape.set", "file.open", "font.add", "insert.audio", "edit.copy"] {
        assert_eq!(a.office_call("office.execute", &json!({"command":command}), None).err().unwrap().code, "UNSUPPORTED");
    }
    assert_eq!(a.office_call("office.export", &json!({"format":"pdf"}), None).err().unwrap().code, "UNSUPPORTED");
    assert_eq!(a.office_call("office.open", &json!({"name":"../bad.pptx"}), Some(&pptx())).err().unwrap().code, "INVALID_ARGUMENT");
    assert_eq!(a.office_revision(), before);
    assert!(!a.office_has_unsaved_documents());
}

#[test]
fn ui_pdf_and_png_copies_wait_for_ack_without_saving_the_source() {
    let (mut a, saves, got) = app();
    edit(&mut a);
    let uid = uid(&a);
    let revision = a.office_revision();
    assert!(a.export_as("pdf", json!({})));
    assert!(a.export_as("png", json!({"transparentBackground":true})));
    assert_eq!(got.borrow().len(), 2);
    {
        let got = got.borrow();
        assert!(got[0].4.ends_with(".pdf"));
        assert!(got[0].5.starts_with(b"%PDF-"));
        assert!(got[0].6);
        assert!(got[1].4.ends_with(".png"));
        let image = deckcraft_render::decode_image(&got[1].5).unwrap();
        assert_eq!(image.get_pixel(2, 2).0[3], 0);
        assert!(got[1].6);
        assert_eq!(got[0].1, uid);
        assert_eq!(got[0].3, revision);
        assert_eq!(got[1].3, revision);
    }
    assert!(a.office_has_pending_saves());
    assert!(a.run("file.close", json!({})).is_err());
    assert_eq!(a.office_call("office.close", &json!({"discardChanges":true}), None).err().unwrap().code, "UNSAVED_CHANGES");
    let ids: Vec<_> = got.borrow().iter().map(|g| g.0).collect();
    saves.finish(ids[0], SaveOutcome::Saved);
    a.office_poll_saves();
    assert!(a.office_has_pending_saves());
    saves.finish(ids[1], SaveOutcome::Saved);
    a.office_poll_saves();
    assert!(!a.office_has_pending_saves());
    assert!(a.session.doc().unwrap().is_dirty());
    assert!(a.session.doc().unwrap().path.is_none());
    assert_eq!(a.office_revision(), revision);
}
#[test]
fn clean_source_copy_success_failure_cancel_and_late_ack_preserve_path_and_dirty() {
    let (mut a, saves, got) = app();
    a.save();
    saves.finish(ticket(&got), SaveOutcome::Saved);
    a.office_poll_saves();
    let path = a.session.doc().unwrap().path.clone();
    for outcome in [SaveOutcome::Saved, SaveOutcome::Failed("denied".into()), SaveOutcome::Cancelled] {
        assert!(a.export_as("pdf", json!({})));
        let ticket = ticket(&got);
        assert!(!a.session.doc().unwrap().is_dirty());
        assert!(a.office_has_pending_saves());
        saves.finish(ticket, outcome);
        a.office_poll_saves();
        saves.finish(ticket, SaveOutcome::Saved);
        a.office_poll_saves();
        assert!(!a.session.doc().unwrap().is_dirty());
        assert_eq!(a.session.doc().unwrap().path, path);
        assert!(!a.office_has_pending_saves());
    }
}
#[test]
fn old_copy_ack_cannot_save_newer_edits_or_retarget_a_parked_owner() {
    let (mut a, saves, got) = app();
    assert!(a.export_as("pdf", json!({})));
    let old = ticket(&got);
    let owner = uid(&a);
    edit(&mut a);
    a.run("file.new", json!({})).unwrap();
    let active = uid(&a);
    saves.finish(old, SaveOutcome::Saved);
    a.office_poll_saves();
    assert_eq!(uid(&a), active);
    assert!(!a.session.doc().unwrap().is_dirty());
    let parked = a.session.documents().iter().find(|d| d.uid == owner).unwrap();
    assert!(parked.is_dirty());
    assert!(parked.path.is_none());
}
fn harness(app: SlideApp) -> egui_kittest::Harness<'static, SlideApp> {
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut SlideApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h
}
#[test]
fn actual_human_close_save_waits_for_two_same_revision_acks() {
    let (mut a, saves, got) = app();
    edit(&mut a);
    let mut h = harness(a);
    h.state_mut().close_presentation();
    h.run_steps(2);
    assert_eq!(h.state().dialog.as_ref().unwrap().id, "close");
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let first = ticket(&got);
    assert!(h.state().dialog.is_none());
    assert!(h.state().office_has_pending_saves());
    call(h.state_mut(), "office.save", json!({}));
    let second = ticket(&got);
    assert_ne!(first, second);
    saves.finish(first, SaveOutcome::Saved);
    h.run_steps(3);
    assert_eq!(h.state().session.documents().len(), 1);
    saves.finish(second, SaveOutcome::Saved);
    h.run_steps(3);
    assert!(h.state().session.documents().is_empty());
}
#[test]
fn deferred_human_close_is_cancelled_by_later_edits_or_failed_owned_write() {
    for newer_edit in [false, true] {
        let (mut a, saves, got) = app();
        edit(&mut a);
        a.office_save_and_close().unwrap();
        let first = ticket(&got);
        call(&mut a, "office.save", json!({}));
        let second = ticket(&got);
        saves.finish(first, SaveOutcome::Saved);
        a.office_poll_saves();
        assert_eq!(a.session.documents().len(), 1);
        if newer_edit {
            edit(&mut a);
            saves.finish(second, SaveOutcome::Saved);
        } else {
            saves.finish(second, SaveOutcome::Failed("denied".into()));
        }
        a.office_poll_saves();
        assert_eq!(a.session.documents().len(), 1);
        assert!(a.session.doc().unwrap().is_dirty());
    }
}
#[test]
fn deferred_close_of_parked_owner_preserves_the_active_window() {
    let (mut a, saves, got) = app();
    edit(&mut a);
    let owner = uid(&a);
    a.office_save_and_close().unwrap();
    let first = ticket(&got);
    a.run("file.new", json!({})).unwrap();
    let active = uid(&a);
    saves.finish(first, SaveOutcome::Saved);
    a.office_poll_saves();
    assert_eq!(uid(&a), active);
    assert!(a.session.documents().iter().all(|d| d.uid != owner));
}

#[test]
fn deferred_close_waits_for_its_ticket_when_same_revision_ack_order_reverses() {
    let (mut a, saves, got) = app();
    edit(&mut a);
    a.office_save_and_close().unwrap();
    let first = ticket(&got);
    call(&mut a, "office.save", json!({}));
    let second = ticket(&got);
    saves.finish(second, SaveOutcome::Saved);
    a.office_poll_saves();
    assert_eq!(a.session.documents().len(), 1);
    saves.finish(first, SaveOutcome::Saved);
    a.office_poll_saves();
    assert!(a.session.documents().is_empty());
}
#[test]
fn a_failed_close_save_is_not_replaced_by_another_successful_save() {
    let (mut a, saves, got) = app();
    edit(&mut a);
    a.office_save_and_close().unwrap();
    let first = ticket(&got);
    call(&mut a, "office.save", json!({}));
    let second = ticket(&got);
    saves.finish(first, SaveOutcome::Failed("denied".into()));
    saves.finish(second, SaveOutcome::Saved);
    a.office_poll_saves();
    assert_eq!(a.session.documents().len(), 1);
    assert!(!a.office_has_pending_saves());
}

#[test]
fn human_save_and_save_as_show_immediate_host_failure() {
    let (mut a, _, _) = app();
    a.services.host_save_versioned = Some(Box::new(|_, _, _, _, _, _, _| Err("host unavailable".into())));
    a.save();
    assert_eq!(a.status.as_ref().unwrap().0, "host unavailable");
    assert!(a.session.doc().unwrap().is_dirty());
    a.status = None;
    a.save_as_dialog("pptx");
    assert_eq!(a.status.as_ref().unwrap().0, "host unavailable");
    assert!(a.session.doc().unwrap().path.is_none());
}

fn comment_harness() -> (egui_kittest::Harness<'static, SlideApp>, AsyncSaves, Got) {
    let (mut a, saves, got) = app();
    a.run("app.dialog", json!({"id":"comment"})).unwrap();
    let mut h = harness(a);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Comment draft").click();
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Comment draft").type_text("Owned human comment draft");
    h.run_steps(3);
    assert_eq!(h.state().dialog.as_ref().unwrap().fields["text"], "Owned human comment draft");
    (h, saves, got)
}
#[test]
fn draft_critic_repro_blocks_replacement_close_edit_export_and_save_until_post() {
    let (mut h, _, got) = comment_harness();
    let owner = uid(h.state());
    let revision = h.state_mut().office_revision();
    assert!(h.state().office_has_unsaved_documents());
    for (method, params, bytes) in [
        ("office.open", json!({"name":"replacement.pptx"}), Some(pptx())),
        ("office.close", json!({"discardChanges":true}), None),
        ("office.execute", json!({"command":"slide.new"}), None),
        ("office.export", json!({"format":"pptx"}), None),
        ("office.save", json!({}), None),
    ] {
        assert_eq!(h.state_mut().office_call(method, &params, bytes.as_deref()).err().unwrap().code, "UNCOMMITTED_DRAFT");
    }
    assert!(h.state_mut().run("file.new", json!({})).is_err());
    assert!(!h.state_mut().export_as("pdf", json!({})));
    h.state_mut().save();
    assert!(got.borrow().is_empty());
    assert_eq!(uid(h.state()), owner);
    assert_eq!(h.state_mut().office_revision(), revision);
    h.get_by_label("Post").click_accesskit();
    h.run_steps(3);
    assert!(!h.state().office_has_drafts());
    assert_eq!(h.state().session.doc().unwrap().doc.slides[0].comments[0].text, "Owned human comment draft");
    assert_eq!(uid(h.state()), owner);
    edit(h.state_mut());
    h.state_mut().save();
    assert_eq!(got.borrow().len(), 1);
}
#[test]
fn draft_cancel_and_keyboard_discard_restore_open_and_saves() {
    for keyboard in [false, true] {
        let (mut h, saves, got) = comment_harness();
        let owner = uid(h.state());
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::N);
        h.run_steps(2);
        assert_eq!(uid(h.state()), owner);
        assert!(h.state().office_has_drafts());
        if keyboard {
            h.key_press(egui::Key::Escape);
        } else {
            h.get_by_label("Cancel").click_accesskit();
        }
        h.run_steps(3);
        assert!(!h.state().office_has_drafts());
        assert!(h.state().session.doc().unwrap().doc.slides[0].comments.is_empty());
        h.state_mut().save();
        saves.finish(ticket(&got), SaveOutcome::Saved);
        h.run_steps(2);
        let opened = h.state_mut().office_call("office.open", &json!({"name":"AfterCancel.pptx"}), Some(&pptx())).unwrap();
        assert_eq!(json_reply(opened)["confirmed"], true);
    }
}
#[test]
fn draft_parked_dialog_cannot_post_to_a_new_window_and_returns_to_original_owner() {
    let (mut h, _, _) = comment_harness();
    let owner = uid(h.state());
    h.state_mut().session.execute("file.new", &json!({})).unwrap();
    let other = uid(h.state());
    h.run_steps(3);
    assert_ne!(owner, other);
    assert!(h.query_by_label("Post").is_none());
    assert_eq!(h.state().dialog.as_ref().unwrap().fields["text"], "Owned human comment draft");
    h.get_by_label("Return to draft owner").click_accesskit();
    h.run_steps(3);
    assert_eq!(uid(h.state()), owner);
    h.get_by_label("Post").click_accesskit();
    h.run_steps(3);
    assert_eq!(h.state().session.documents().iter().find(|d| d.uid == owner).unwrap().doc.slides[0].comments.len(), 1);
    assert!(h.state().session.documents().iter().find(|d| d.uid == other).unwrap().doc.slides[0].comments.is_empty());
}
#[test]
fn draft_history_change_requires_explicit_resume_on_original_target() {
    let (mut h, _, _) = comment_harness();
    let owner = uid(h.state());
    let target = h.state().session.doc().unwrap().doc.slides[0].id;
    h.state_mut().session.execute("slide.new", &json!({})).unwrap();
    h.state_mut().session.execute("edit.undo", &json!({})).unwrap();
    h.run_steps(2);
    assert!(h.query_by_label("Post").is_none());
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(h.state().session.doc().unwrap().doc.slides[0].comments.is_empty());
    h.get_by_label("Resume on original target").click_accesskit();
    h.run_steps(3);
    h.get_by_label("Post").click_accesskit();
    h.run_steps(3);
    assert_eq!(uid(h.state()), owner);
    assert_eq!(h.state().session.doc().unwrap().doc.slides[0].id, target);
    assert_eq!(h.state().session.doc().unwrap().doc.slides[0].comments.len(), 1);
}
#[test]
fn draft_deleted_target_can_be_recovered_with_explicit_history_redo() {
    let (mut a, _, _) = app();
    edit(&mut a);
    let target = a.session.doc().unwrap().current_slide().unwrap().id;
    a.run("app.dialog", json!({"id":"comment"})).unwrap();
    let mut h = harness(a);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Comment draft").click();
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Comment draft").type_text("Recovered target comment");
    h.run_steps(2);
    h.state_mut().session.execute("edit.undo", &json!({})).unwrap();
    h.run_steps(2);
    h.get_by_label("Resume on original target").click_accesskit();
    h.run_steps(2);
    assert!(h.query_by_label("Post").is_none());
    h.get_by_label("Redo in original presentation").click_accesskit();
    h.run_steps(2);
    h.get_by_label("Resume on original target").click_accesskit();
    h.run_steps(3);
    h.get_by_label("Post").click_accesskit();
    h.run_steps(3);
    let doc = &h.state().session.doc().unwrap().doc;
    let slide = doc.slides.iter().find(|s| s.id == target).unwrap();
    assert_eq!(slide.comments[0].text, "Recovered target comment");
}
#[test]
fn draft_reply_post_discard_and_parked_owner_never_share_index_only_text() {
    let (mut a, _, _) = app();
    a.run("comment.add", json!({"text":"Original comment"})).unwrap();
    a.ui.pane = Some("comments".into());
    let owner = uid(&a);
    let mut h = harness(a);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Reply to comment 1 on slide 1").click();
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Reply to comment 1 on slide 1").type_text("Owned reply");
    h.run_steps(2);
    assert!(h.state().office_has_drafts());
    let key = h.state().office.drafts.text.keys().next().unwrap().clone();
    h.state_mut().session.execute("file.new", &json!({})).unwrap();
    let other = uid(h.state());
    h.state_mut().session.execute("comment.add", &json!({"text":"Other comment"})).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().office.drafts.text[&key].text, "Owned reply");
    assert!(h.state().session.doc().unwrap().doc.slides[0].comments[0].replies.is_empty());
    h.state_mut().office_resume_draft(&key).unwrap();
    h.run_steps(3);
    assert_eq!(uid(h.state()), owner);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Reply to comment 1 on slide 1").click();
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert!(!h.state().office_has_drafts());
    assert_eq!(h.state().session.doc().unwrap().doc.slides[0].comments[0].replies[0].text, "Owned reply");
    assert!(h.state().session.documents().iter().find(|d| d.uid == other).unwrap().doc.slides[0].comments[0].replies.is_empty());
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Reply to comment 1 on slide 1").click();
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Reply to comment 1 on slide 1").type_text("Discard reply");
    h.run_steps(2);
    h.get_by_label("Discard reply").click_accesskit();
    h.run_steps(2);
    assert!(!h.state().office_has_drafts());
}
#[test]
fn draft_other_modal_and_bounded_inline_work_guard_source_and_copy_tickets() {
    let (mut a, saves, got) = app();
    a.save();
    let pending = ticket(&got);
    a.run("app.dialog", json!({"id":"altText"})).unwrap();
    a.dialog.as_mut().unwrap().fields.insert("text".into(), "Uncommitted description".into());
    a.office_revision();
    saves.finish(pending, SaveOutcome::Saved);
    a.office_poll_saves();
    assert!(a.office_has_unsaved_documents());
    assert_eq!(a.office_call("office.export", &json!({}), None).err().unwrap().code, "UNCOMMITTED_DRAFT");
    a.dialog = None;
    let owner = uid(&a);
    assert!(a.office_record_draft(&format!("property:{owner}:title"), "", "Draft property"));
    assert!(a.office_has_drafts());
    a.session.execute("file.new", &json!({})).unwrap();
    assert!(a.office_commit_draft(&format!("property:{owner}:title"), "file.properties", json!({"title":"wrong"})).is_err());
    a.office_discard_draft(&format!("property:{owner}:title"));
    assert!(!a.office_has_drafts());
    assert!(!a.office_record_draft("large", "", &"x".repeat(crate::office::drafts::MAX_DRAFT_TEXT + 1)));
}

#[test]
fn draft_actual_notes_input_blocks_host_then_commits_to_its_owner_on_blur() {
    let (mut a, saves, got) = app();
    a.ui.notes = true;
    let mut h = harness(a);
    let owner = uid(h.state());
    let note_id = egui::Id::unique(("notes_edit", owner, h.state().session.doc().unwrap().current_slide().map(|s| s.id.0)));
    h.ctx.memory_mut(|m| m.request_focus(note_id));
    h.run_steps(2);
    h.input_mut().events.push(egui::Event::Text("Owned notes draft".into()));
    h.run_steps(2);
    assert!(h.state().office_has_drafts());
    assert!(h.state().notes_buf.2.text.contains("Owned notes draft"));
    assert_eq!(h.state_mut().office_call("office.open", &json!({"name":"wrong.pptx"}), Some(&pptx())).err().unwrap().code, "UNCOMMITTED_DRAFT");
    h.ctx.memory_mut(|m| m.surrender_focus(note_id));
    h.run_steps(3);
    assert!(!h.state().office_has_drafts());
    assert_eq!(uid(h.state()), owner);
    assert!(h.state().session.doc().unwrap().doc.slides[0].notes.text().contains("Owned notes draft"));
    h.state_mut().save();
    saves.finish(ticket(&got), SaveOutcome::Saved);
    h.run_steps(2);
    assert!(!h.state().office_has_unsaved_documents());
}
#[test]
fn draft_actual_property_input_survives_parked_owner_and_commits_after_explicit_resume() {
    let (mut a, _, _) = app();
    a.run("app.backstage", json!({"page":"info"})).unwrap();
    let owner = uid(&a);
    let mut h = harness(a);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Author").click();
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Author").type_text("Owned property draft");
    h.run_steps(2);
    assert!(h.state().office_has_drafts());
    let key = format!("property:{owner}:author");
    assert_eq!(h.state().office.drafts.text[&key].text, "Owned property draft");
    h.state_mut().session.execute("file.new", &json!({})).unwrap();
    let other = uid(h.state());
    h.run_steps(3);
    assert!(h.state().session.doc().unwrap().doc.props.author.is_empty());
    assert!(h.state().office_has_drafts());
    h.state_mut().office_resume_draft(&key).unwrap();
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Author").click();
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(uid(h.state()), owner);
    assert!(!h.state().office_has_drafts());
    assert_eq!(h.state().session.doc().unwrap().doc.props.author, "Owned property draft");
    assert!(h.state().session.documents().iter().find(|d| d.uid == other).unwrap().doc.props.author.is_empty());
}
#[test]
fn draft_same_frame_human_input_and_close_ack_preserves_uncommitted_comment() {
    let (mut a, saves, got) = app();
    edit(&mut a);
    let owner = uid(&a);
    a.office_save_and_close().unwrap();
    let save = ticket(&got);
    a.run("app.dialog", json!({"id":"comment"})).unwrap();
    let ack: Rc<std::cell::Cell<Option<u64>>> = Rc::new(std::cell::Cell::new(None));
    let ack_frame = ack.clone();
    let pool = saves.clone();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_ui_state(
        move |ui, app: &mut SlideApp| {
            app.logic(ui.ctx());
            app.ui(ui);
            // The web shell's safe post-UI dispatch phase accepts queued ACKs only after this
            // same frame's actual keyboard events have been applied to their owning draft.
            if let Some(ticket) = ack_frame.take() {
                pool.finish(ticket, SaveOutcome::Saved);
                app.office_poll_saves();
            }
        },
        a,
    );
    h.run_steps(3);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Comment draft").click();
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Comment draft").type_text("Typing concurrent with close ACK");
    ack.set(Some(save));
    h.run_steps(1);
    assert_eq!(uid(h.state()), owner);
    assert!(h.state().office_has_unsaved_documents());
    assert_eq!(h.state().dialog.as_ref().unwrap().fields["text"], "Typing concurrent with close ACK");
    h.get_by_label("Post").click_accesskit();
    h.run_steps(3);
    assert_eq!(uid(h.state()), owner);
    assert_eq!(h.state().session.doc().unwrap().current_slide().unwrap().comments[0].text, "Typing concurrent with close ACK");
    assert!(h.state().session.doc().unwrap().is_dirty());
}

#[test]
fn draft_keyboard_undo_history_does_not_leak_between_cancelled_dialog_owners() {
    let (mut h, _, _) = comment_harness();
    let original = uid(h.state());
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    h.state_mut().office_call("office.open", &json!({"name":"FreshKeyboardOwner.pptx"}), Some(&pptx())).unwrap();
    let owner = uid(h.state());
    assert_ne!(original, owner);
    h.state_mut().run("app.dialog", json!({"id":"comment"})).unwrap();
    h.run_steps(3);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Comment draft").click();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run_steps(2);
    assert_eq!(h.state().dialog.as_ref().unwrap().fields["text"], "");
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Comment draft").type_text("New owner only");
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run_steps(2);
    assert_eq!(h.state().dialog.as_ref().unwrap().fields["text"], "");
    h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z);
    h.run_steps(2);
    assert_eq!(h.state().dialog.as_ref().unwrap().fields["text"], "New owner only");
    h.get_by_label("Post").click_accesskit();
    h.run_steps(2);
    assert_eq!(uid(h.state()), owner);
    assert_eq!(h.state().session.doc().unwrap().doc.slides[0].comments[0].text, "New owner only");
}

#[test]
fn draft_reply_history_cannot_reuse_a_replaced_comment_index() {
    let (mut a, _, _) = app();
    a.run("comment.add", json!({"text":"Original reply target"})).unwrap();
    a.ui.pane = Some("comments".into());
    let mut h = harness(a);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Reply to comment 1 on slide 1").click();
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Reply to comment 1 on slide 1").type_text("Reply retained across history");
    h.run_steps(2);
    let key = h.state().office.drafts.text.keys().next().unwrap().clone();
    h.state_mut().session.execute("comment.delete", &json!({"index":0})).unwrap();
    h.state_mut().session.execute("comment.add", &json!({"text":"Replacement at same index"})).unwrap();
    h.run_steps(2);
    assert!(h.state_mut().office_resume_draft(&key).is_err());
    assert!(h.state_mut().office_commit_draft(&key, "comment.reply", json!({"index":0,"text":"Wrong"})).is_err());
    assert!(h.state().session.doc().unwrap().doc.slides[0].comments[0].replies.is_empty());
    h.state_mut().session.execute("edit.undo", &json!({})).unwrap();
    h.state_mut().session.execute("edit.undo", &json!({})).unwrap();
    h.state_mut().office_resume_draft(&key).unwrap();
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Reply to comment 1 on slide 1").click();
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    let comment = &h.state().session.doc().unwrap().doc.slides[0].comments[0];
    assert_eq!(comment.text, "Original reply target");
    assert_eq!(comment.replies[0].text, "Reply retained across history");
}
#[test]
fn draft_notes_page_is_retained_for_parked_owner_and_never_reseeded_to_other_notes() {
    let (mut a, _, _) = app();
    a.ui.view = crate::ViewMode::NotesPage;
    let owner = uid(&a);
    let mut h = harness(a);
    let edit_id = egui::Id::unique(("notespage-edit", owner, h.state().session.doc().unwrap().current_slide().map(|s| s.id.0)));
    h.ctx.memory_mut(|m| m.request_focus(edit_id));
    h.run_steps(2);
    h.input_mut().events.push(egui::Event::Text("Owned notes page draft".into()));
    h.run_steps(2);
    assert!(h.state().office_has_drafts());
    let key = h.state().office.drafts.text.keys().next().unwrap().clone();
    assert!(key.starts_with("notespage:"));
    h.state_mut().session.execute("file.new", &json!({})).unwrap();
    let other = uid(h.state());
    h.run_steps(3);
    assert!(h.state().session.doc().unwrap().doc.slides[0].notes.text().is_empty());
    h.state_mut().office_resume_draft(&key).unwrap();
    h.run_steps(3);
    assert_eq!(uid(h.state()), owner);
    assert!(!h.state().office_has_drafts());
    assert!(h.state().session.doc().unwrap().doc.slides[0].notes.text().contains("Owned notes page draft"));
    assert!(h.state().session.documents().iter().find(|d| d.uid == other).unwrap().doc.slides[0].notes.text().is_empty());
}

#[test]
fn draft_recovery_reopens_property_editor_and_discard_does_not_recreate_text() {
    let (mut a, _, _) = app();
    a.run("app.backstage", json!({"page":"info"})).unwrap();
    let mut h = harness(a);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Author").click();
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Author").type_text("Recoverable property");
    h.run_steps(2);
    h.state_mut().close_backstage();
    assert!(h.state().backstage.is_some());
    h.state_mut().backstage = None;
    h.run_steps(2);
    h.get_by_label("Continue editing draft").click_accesskit();
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Author").click();
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(!h.state().office_has_drafts());
    assert_eq!(h.state().session.doc().unwrap().doc.props.author, "Recoverable property");
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Author").click();
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Author").type_text("Discard me");
    h.run_steps(2);
    assert!(h.state().office_has_drafts());
    h.get_by_label("Discard draft").click_accesskit();
    h.run_steps(3);
    assert!(!h.state().office_has_drafts());
    assert_eq!(h.state().session.doc().unwrap().doc.props.author, "Recoverable property");
}
