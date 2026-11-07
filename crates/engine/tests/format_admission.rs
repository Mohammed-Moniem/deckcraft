//! File-command format admission must precede file and saved-document effects.

use std::path::PathBuf;
use std::sync::Arc;

use deckcraft_engine::{
    Session,
    cmd::file::{WRITABLE_EXTENSIONS, save_bytes},
};
use serde_json::json;

struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("deckcraft-format-admission-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dirty_session() -> Session {
    let mut s = Session::with_new();
    s.execute("file.properties", &json!({"title": "Format admission sentinel"})).unwrap();
    assert!(s.doc().unwrap().is_dirty());
    s
}

fn unchanged(s: &Session, before: &deckcraft_engine::DocState) -> bool {
    let after = s.doc().unwrap();
    Arc::ptr_eq(&after.doc, &before.doc)
        && Arc::ptr_eq(&after.saved_doc, &before.saved_doc)
        && after.path == before.path
        && after.revision == before.revision
        && after.is_dirty() == before.is_dirty()
}

#[test]
fn unknown_explicit_save_formats_are_errors() {
    let mut s = dirty_session();
    let before = s.doc().unwrap().clone();
    let mut accepted = vec![];
    for format in ["odp", "svg", "", "future-format"] {
        let direct = save_bytes(&before.doc, format);
        let command = s.execute("file.saveBytes", &json!({"format": format}));
        println!(
            "format={format:?} direct_ok={} saveBytes_ok={} native_fallback={}",
            direct.is_ok(),
            command.is_ok(),
            direct.as_ref().is_ok_and(|bytes| deckcraft_format::sniff(bytes))
        );
        if direct.is_ok() || command.is_ok() {
            accepted.push(format);
        }
        assert!(unchanged(&s, &before));
    }
    assert!(accepted.is_empty(), "unsupported formats accepted: {accepted:?}");
}

#[test]
fn unsupported_inferred_paths_leave_files_and_document_unchanged() {
    let dir = Scratch::new("inferred");
    let mut failures = vec![];
    for command in ["file.save", "file.saveAs", "file.saveTemplate", "file.export"] {
        for name in ["deck.odp", "deck.svg", "deck.bmp", "deck.outline", "deck", "deck."] {
            for exists in [false, true] {
                let mut s = dirty_session();
                let before = s.doc().unwrap().clone();
                let path = dir.0.join(name);
                let _ = std::fs::remove_file(&path);
                if exists {
                    std::fs::write(&path, b"existing destination sentinel").unwrap();
                }
                let result = s.execute(command, &json!({"path": path}));
                let file_unchanged = if exists { std::fs::read(&path).unwrap() == b"existing destination sentinel" } else { !path.exists() };
                let state_unchanged = unchanged(&s, &before);
                println!(
                    "{command} {name} exists={exists}: accepted={} file_unchanged={file_unchanged} state_unchanged={state_unchanged}",
                    result.is_ok()
                );
                if result.is_ok() || !file_unchanged || !state_unchanged {
                    failures.push(format!("{command} {name} exists={exists}: {result:?}"));
                } else {
                    assert!(result.unwrap_err().to_string().contains("unsupported output format"));
                }
                let _ = std::fs::remove_file(&path);
                assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0, "no temporary files may remain");
            }
        }
    }
    assert!(failures.is_empty(), "{} unsupported outputs were accepted or changed state: {failures:?}", failures.len());
}

#[test]
fn inferred_rejection_precedes_io_and_preserves_existing_save_path_and_recovery() {
    let dir = Scratch::new("existing-path");
    let path = dir.0.join("existing.odp");
    std::fs::write(&path, b"existing destination sentinel").unwrap();
    let mut s = dirty_session();
    s.doc_mut().unwrap().path = Some(path.to_string_lossy().into_owned());
    s.recovery_dir = Some(dir.0.join("recovery"));
    s.execute("file.recovery.save", &json!({})).unwrap();
    let recovered = s.execute("file.recovery.list", &json!({})).unwrap();
    assert_eq!(recovered.as_array().unwrap().len(), 1);
    let before = s.doc().unwrap().clone();
    let result = s.execute("file.save", &json!({}));
    println!("save existing unsupported path: {result:?}");
    assert!(result.is_err());
    assert!(unchanged(&s, &before));
    assert_eq!(std::fs::read(&path).unwrap(), b"existing destination sentinel");
    assert_eq!(s.execute("file.recovery.list", &json!({})).unwrap(), recovered);
    for command in ["file.save", "file.export"] {
        let error = s.execute(command, &json!({"path": dir.0.join("missing-parent/deck.svg")})).unwrap_err();
        assert!(error.to_string().contains("unsupported output format"), "{error}");
    }
}

#[test]
fn explicit_unknown_file_formats_do_not_write_or_mark_saved() {
    let dir = Scratch::new("explicit-unknown");
    for command in ["file.save", "file.saveAs", "file.saveTemplate", "file.export"] {
        let mut s = dirty_session();
        let before = s.doc().unwrap().clone();
        let path = dir.0.join("valid.deckcraft");
        std::fs::write(&path, b"sentinel").unwrap();
        assert!(s.execute(command, &json!({"path": path, "format": "odp"})).is_err(), "{command}");
        assert_eq!(std::fs::read(path).unwrap(), b"sentinel");
        assert!(unchanged(&s, &before));
    }
}

#[test]
fn format_rejection_precedes_native_serialization() {
    let dir = Scratch::new("serialization");
    let mut s = dirty_session();
    // Repeated media IDs would create duplicate ZIP members and are rejected by the native writer.
    Arc::make_mut(&mut s.doc_mut().unwrap().doc).media = vec![deckcraft_model::MediaItem::default(); 2];
    let before = s.doc().unwrap().clone();
    // Positive control: this document reaches and fails the native serializer.
    assert!(save_bytes(&before.doc, "deckcraft").is_err());
    let error = save_bytes(&before.doc, "odp").unwrap_err().to_string();
    assert!(error.contains("unknown format"), "format admission must win over serialization: {error}");
    for command in ["file.save", "file.export"] {
        let error = s.execute(command, &json!({"path": dir.0.join("deck.odp")})).unwrap_err().to_string();
        assert!(error.contains("unsupported output format"), "{command}: {error}");
        assert!(unchanged(&s, &before));
    }
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
}

#[test]
fn save_without_a_path_still_requests_the_picker() {
    let mut s = dirty_session();
    let before = s.doc().unwrap().clone();
    for command in ["file.save", "file.saveAs", "file.saveTemplate"] {
        assert_eq!(s.execute(command, &json!({})).unwrap()["needsPath"], true);
        assert!(unchanged(&s, &before));
    }
}

#[test]
fn legacy_native_name_can_be_opened_edited_and_saved() {
    let dir = Scratch::new("legacy-native");
    for name in ["legacy.slidecraft", "legacy.SLIDECRAFT"] {
        let path = dir.0.join(name);
        let initial = Session::with_new();
        std::fs::write(&path, save_bytes(&initial.doc().unwrap().doc, "deckcraft").unwrap()).unwrap();
        let mut s = Session::new();
        s.execute("file.open", &json!({"path": path})).unwrap();
        assert_eq!(s.doc().unwrap().path.as_deref(), path.to_str());
        assert!(!s.doc().unwrap().is_dirty());
        s.execute("file.properties", &json!({"title": "Legacy save edited"})).unwrap();
        assert!(s.doc().unwrap().is_dirty());
        let revision = s.doc().unwrap().revision;
        s.execute("file.save", &json!({})).unwrap();
        assert!(!s.doc().unwrap().is_dirty());
        assert_eq!(s.doc().unwrap().revision, revision + 1);
        assert_eq!(s.doc().unwrap().path.as_deref(), path.to_str());
        assert_eq!(deckcraft_format::load(&std::fs::read(&path).unwrap()).unwrap().props.title, "Legacy save edited");
    }
}

fn check_bytes(format: &str, bytes: &[u8]) {
    assert!(!bytes.is_empty(), "{format}");
    match format {
        "deckcraft" | "slidecraft" => assert_eq!(deckcraft_format::load(bytes).unwrap().props.title, "Format admission sentinel"),
        "pptx" | "potx" | "ppsx" => {
            assert_eq!(deckcraft_pptx::import(bytes).unwrap().props.title, "Format admission sentinel");
            let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
            let mut types = String::new();
            std::io::Read::read_to_string(&mut zip.by_name("[Content_Types].xml").unwrap(), &mut types).unwrap();
            let kind = match format {
                "potx" => "template",
                "ppsx" => "slideshow",
                _ => "presentation",
            };
            assert!(types.contains(&format!("presentationml.{kind}.main+xml")), "{format}: {types}");
        }
        "pdf" => assert!(bytes.starts_with(b"%PDF-")),
        "png" => assert_eq!(image::guess_format(bytes).unwrap(), image::ImageFormat::Png),
        "jpg" | "jpeg" => assert_eq!(image::guess_format(bytes).unwrap(), image::ImageFormat::Jpeg),
        "gif" => assert_eq!(image::guess_format(bytes).unwrap(), image::ImageFormat::Gif),
        "rtf" => assert!(deckcraft_format::rtf::sniff_rtf(bytes)),
        "txt" | "md" | "outline" => assert!(std::str::from_utf8(bytes).is_ok()),
        _ => panic!("unverified writer {format}"),
    }
}

#[test]
fn documented_byte_writers_and_default_native_still_work() {
    let mut s = dirty_session();
    let before = s.doc().unwrap().clone();
    for format in ["deckcraft", "pptx", "potx", "ppsx", "outline", "txt", "rtf", "pdf", "gif"] {
        check_bytes(format, &save_bytes(&before.doc, format).unwrap());
        assert!(s.execute("file.saveBytes", &json!({"format": format})).is_ok(), "{format}");
        assert!(unchanged(&s, &before));
    }
    assert!(s.execute("file.saveBytes", &json!({})).is_ok());
    for format in ["png", "jpeg", "jpg"] {
        assert!(save_bytes(&before.doc, format).is_err());
    }
}

#[test]
fn all_writable_export_extensions_use_their_documented_writer() {
    let dir = Scratch::new("writers");
    let mut s = dirty_session();
    let before = s.doc().unwrap().clone();
    for ext in WRITABLE_EXTENSIONS {
        let path = dir.0.join(format!("deck.{}", ext.to_uppercase()));
        s.execute("file.export", &json!({"path": path, "scale": 0.05, "width": 16, "secondsPerSlide": 0.1})).unwrap();
        check_bytes(ext, &std::fs::read(&path).unwrap());
        assert!(unchanged(&s, &before), "export {ext} must not mark the document saved");
    }
}

#[test]
fn native_and_pptx_saves_mark_saved_and_explicit_overrides_remain_available() {
    let dir = Scratch::new("saves");
    for command in ["file.save", "file.saveAs", "file.saveTemplate"] {
        for format in ["deckcraft", "pptx"] {
            for explicit in [false, true] {
                let mut s = dirty_session();
                let revision = s.doc().unwrap().revision;
                let path = dir.0.join(if explicit { "custom-name".to_string() } else { format!("deck.{format}") });
                let mut params = json!({"path": path});
                if explicit {
                    params["format"] = json!(format);
                }
                s.execute(command, &params).unwrap();
                check_bytes(format, &std::fs::read(&path).unwrap());
                assert!(!s.doc().unwrap().is_dirty());
                assert_eq!(s.doc().unwrap().revision, revision + 1);
                assert_eq!(s.doc().unwrap().path.as_deref(), path.to_str());
            }
        }
    }
    let mut s = dirty_session();
    let before = s.doc().unwrap().clone();
    for format in ["deckcraft", "pptx", "outline", "rtf", "pdf", "png", "jpeg", "gif"] {
        let path = dir.0.join("explicit.svg");
        s.execute("file.export", &json!({"path": path, "format": format, "scale": 0.05, "width": 16})).unwrap();
        check_bytes(format, &std::fs::read(path).unwrap());
        assert!(unchanged(&s, &before));
    }
}
