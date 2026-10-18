//! End-to-end tests of the delete dialogs, cleanups and the journal window. Every tree is a
//! `tempfile` directory and the platform is `FakePlatform` (a private "recycle bin" in a temp dir),
//! so no test touches real user data, the real recycle bin or the real file manager.

use super::*;
use crate::cleanups_ui::{self, ActionChoice, CleanupForm, CleanupsAction, KindChoice};
use crate::delete_dialog::{self, DeleteDialog, DeleteMode, Hint, Subject};
use dirustat_platform::{FakePlatform, PlatformCall};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

struct Rig {
    app: DirustatApp,
    fake: Arc<FakePlatform>,
    /// Keeps the fixture alive.
    dir: tempfile::TempDir,
    ctx: egui::Context,
}

fn screen() -> egui::Rect {
    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 760.0))
}

fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
}

impl Rig {
    /// `root/{docs/{report.txt, old.temp}, big.bin, small.bin, a.tmp}` scanned through the UI.
    fn new(files: usize, exclude: Option<&str>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs").join("report.txt"), vec![0u8; 3000]).unwrap();
        std::fs::write(root.join("docs").join("old.temp"), vec![0u8; 700]).unwrap();
        std::fs::write(root.join("big.bin"), vec![0u8; 9000]).unwrap();
        std::fs::write(root.join("small.bin"), vec![0u8; 10]).unwrap();
        std::fs::write(root.join("a.tmp"), vec![0u8; 40]).unwrap();
        for i in 0..files {
            std::fs::write(root.join(format!("f{i:02}.dat")), b"x").unwrap();
        }
        let fake = Arc::new(FakePlatform::new());
        let mut session = Session::new();
        session.set_platform(fake.clone());
        let mut app = DirustatApp::with_session(session);
        if let Some(pattern) = exclude {
            app.apply_prefs_action(prefs::PrefsAction::Add(pattern.to_owned()));
        }
        let ctx = egui::Context::default();
        app.state_mut().path_text = root.to_string_lossy().into_owned();
        app.start_scan();
        let deadline = Instant::now() + Duration::from_secs(20);
        while app.is_scanning() && Instant::now() < deadline {
            ctx.run_ui(egui::RawInput::default(), |ui| app.show(ui)).drop_without_applying_deltas();
            std::thread::sleep(Duration::from_millis(5));
        }
        app.session().execute("view.size_mode", &json!({ "mode": "logical" })).unwrap();
        let mut rig = Self { app, fake, dir, ctx };
        rig.frames(3, vec![]);
        rig
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("root")
    }

    fn frames(&mut self, n: usize, events: Vec<egui::Event>) {
        for i in 0..n {
            let input = egui::RawInput {
                screen_rect: Some(screen()),
                events: if i == 0 { events.clone() } else { Vec::new() },
                ..egui::RawInput::default()
            };
            let out = self.ctx.run_ui(input, |ui| self.app.show(ui));
            out.drop_without_applying_deltas();
        }
    }

    fn id_of(&self, name: &str) -> usize {
        let session = self.app.session();
        let tree = session.tree();
        let mut stack = vec![tree.root()];
        while let Some(n) = stack.pop() {
            if tree.get(n).is_some_and(|x| x.name() == name) {
                return n.index();
            }
            stack.extend(tree.children(n));
        }
        panic!("no node named {name}");
    }

    fn select(&mut self, name: &str) {
        let id = self.id_of(name);
        self.app.session().execute("select.node", &json!({ "id": id })).unwrap();
    }

    fn dialog(&self) -> DeleteDialog {
        self.app.state().delete_dialog.clone().expect("a dialog is open")
    }

    /// What the confirm button does (the only code path that sends the destructive command).
    fn confirm(&mut self) {
        let mut session = lock_session(&self.app.session);
        delete_dialog::confirm(&mut session, &mut self.app.state);
    }

    fn open(&mut self, name: &str, id: &str) {
        self.select(name);
        self.app.run_command(id).unwrap();
    }

    fn journal_total(&self) -> u64 {
        self.app.session().execute("file.journal", &json!({})).unwrap()["total"].as_u64().unwrap()
    }
}

fn exists(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

#[test]
fn recycle_flow_preview_cancel_then_confirm() {
    let mut r = Rig::new(0, None);
    let big = r.root().join("big.bin");
    r.open("big.bin", "file.delete");
    let d = r.dialog();
    assert_eq!((d.mode, d.count, d.size), (DeleteMode::Recycle, 1, 9000));
    assert!(d.path.ends_with("big.bin") && d.paths.first() == Some(&d.path) && !d.partial_tree);
    // `id` always travels with the revision it was read at.
    let (cmd, params) = d.request();
    assert_eq!(cmd, "file.delete");
    assert_eq!(params["revision"], r.app.session().revision());
    assert_eq!(params["id"], r.id_of("big.bin"));
    assert_eq!(params["confirm"], true);
    assert!(params.get("i_understand_permanent").is_none());
    // Opening only previews: nothing is touched and the dialog draws in both themes.
    for theme in [ThemeKind::Dark, ThemeKind::Light] {
        r.app.state_mut().theme = theme;
        r.frames(2, vec![]);
    }
    assert!(exists(&big) && r.fake.binned().is_empty() && r.journal_total() == 0);
    // Escape cancels.
    r.frames(1, vec![key(egui::Key::Escape, egui::Modifiers::NONE)]);
    r.frames(1, vec![]);
    assert!(r.app.state().delete_dialog.is_none() && exists(&big));
    // Confirm moves it into the fake bin and re-reads the state.
    let revision = r.app.session().revision();
    r.open("big.bin", "file.delete");
    r.confirm();
    assert!(!exists(&big));
    assert_eq!(r.fake.binned().len(), 1);
    assert!(r.app.state().delete_dialog.is_none());
    let toast = r.app.state().toast.clone().expect("a toast reports the outcome");
    assert!(toast.text.contains("Recycle Bin") && toast.text.contains("big.bin"), "{}", toast.text);
    assert!(toast.text.contains("8.7 KiB"), "{}", toast.text);
    assert!(r.app.session().revision() > revision);
    assert!(r.app.session().selection().is_none());
    assert_eq!(r.journal_total(), 1);
    r.frames(2, vec![]);
}

#[test]
fn stale_revision_and_stale_disk_are_friendly_and_delete_nothing() {
    let mut r = Rig::new(0, None);
    let small = r.root().join("small.bin");
    // The tree changes under an open dialog: its id/revision no longer match.
    r.open("small.bin", "file.delete");
    let other = r.root().join("a.tmp").to_string_lossy().into_owned();
    r.app.session().execute("file.delete", &json!({ "path": other, "confirm": true })).unwrap();
    r.confirm();
    let d = r.dialog();
    let error = d.error.expect("refused");
    assert_eq!(error.hint, Hint::Rescan);
    assert!(error.message.contains("no longer matches"), "{}", error.message);
    assert!(exists(&small));
    assert_eq!(r.fake.binned().len(), 1, "only the first, explicit delete happened");
    // The confirm button is gone while an error shows; frames still build.
    assert!(!r.dialog().can_confirm());
    r.frames(2, vec![]);
    // The disk changed after the scan: stale_tree again, with the same offer.
    r.app.state_mut().delete_dialog = None;
    r.open("big.bin", "file.delete");
    std::fs::write(r.root().join("big.bin"), vec![0u8; 20_000]).unwrap();
    r.confirm();
    assert_eq!(r.dialog().error.map(|e| e.hint), Some(Hint::Rescan));
    assert!(exists(&r.root().join("big.bin")));
    // "Scan again" closes the dialog and starts a scan of the same root.
    r.app.state_mut().delete_dialog = None;
    r.app.rescan();
    assert!(r.app.is_scanning() || matches!(r.app.state().scan, ScanStatus::Running { .. } | ScanStatus::Done { .. }));
    let deadline = Instant::now() + Duration::from_secs(20);
    while r.app.is_scanning() && Instant::now() < deadline {
        r.frames(1, vec![]);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn permanent_needs_the_checkbox_and_partial_trees_need_theirs() {
    let mut r = Rig::new(0, Some("*.zzz"));
    let docs = r.root().join("docs");
    r.open("docs", "file.delete_permanent");
    let d = r.dialog();
    assert_eq!(d.mode, DeleteMode::Permanent);
    assert!(d.partial_tree, "an exclusion makes the tree partial");
    assert_eq!(d.count, 3);
    assert!(!d.can_confirm());
    // Without the boxes confirm does nothing at all.
    r.confirm();
    assert!(exists(&docs) && r.dialog().error.is_none());
    r.app.state_mut().delete_dialog.as_mut().unwrap().ack_permanent = true;
    assert!(!r.dialog().can_confirm(), "partial_tree needs its own box");
    r.confirm();
    assert!(exists(&docs));
    r.app.state_mut().delete_dialog.as_mut().unwrap().ack_partial = true;
    let (cmd, params) = r.dialog().request();
    assert_eq!(cmd, "file.delete_permanent");
    assert_eq!(params["i_understand_permanent"], true);
    assert_eq!(params["i_understand_partial"], true);
    for theme in [ThemeKind::Dark, ThemeKind::Light] {
        r.app.state_mut().theme = theme;
        r.frames(2, vec![]);
    }
    r.confirm();
    assert!(!exists(&docs), "deleted for good once both boxes are ticked");
    assert!(r.fake.binned().is_empty(), "permanent deletion never uses the bin");
    let toast = r.app.state().toast.clone().unwrap();
    assert!(toast.text.starts_with("Deleted permanently"), "{}", toast.text);
    assert_eq!(r.app.session().execute("file.journal", &json!({})).unwrap()["entries"][0]["mode"], "permanent");
    // A recycle of a partial folder needs no extra box (the engine does not ask for one).
    let mut r = Rig::new(0, Some("*.zzz"));
    r.open("docs", "file.delete");
    assert!(r.dialog().partial_tree && r.dialog().can_confirm());
}

#[test]
fn unsupported_recycle_offers_the_permanent_flow_without_deleting() {
    let mut r = Rig::new(0, None);
    r.fake.set_no_recycle_bin(true);
    let big = r.root().join("big.bin");
    r.open("big.bin", "file.delete");
    r.confirm();
    let error = r.dialog().error.expect("no recycle bin");
    assert_eq!(error.hint, Hint::OfferPermanent);
    assert!(error.message.contains("no Recycle Bin"), "{}", error.message);
    assert!(exists(&big));
    r.frames(2, vec![]);
    // The offer re-previews as a permanent deletion; nothing is acknowledged yet.
    {
        let mut session = lock_session(&r.app.session);
        delete_dialog::switch_to_permanent(&mut session, &mut r.app.state);
    }
    let d = r.dialog();
    assert_eq!((d.mode, d.error.is_none(), d.ack_permanent), (DeleteMode::Permanent, true, false));
    r.confirm();
    assert!(exists(&big));
    r.app.state_mut().delete_dialog.as_mut().unwrap().ack_permanent = true;
    r.confirm();
    assert!(!exists(&big));
}

#[test]
fn network_scan_roots_disable_every_delete_entry() {
    let mut r = Rig::new(0, None);
    // Relabel the root like a UNC share; the files stay real but are never touched.
    let handle = dirustat_scan::start(&r.root(), dirustat_scan::ScanOptions::default()).unwrap();
    let mut tree = loop {
        if handle.is_finished() {
            break handle.join().unwrap().tree;
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let root = tree.root();
    tree.set_node_name(root, r"\\server\share").unwrap();
    r.app.session().replace_tree(tree);
    r.frames(2, vec![]);
    r.select("big.bin");
    assert_eq!(delete_dialog::block_reason(&r.app.session()), Some(delete_dialog::UNC_REASON));
    let infos = r.app.session().commands();
    let groups = r.app.menu_groups(&infos);
    for id in ["file.delete", "file.delete_permanent"] {
        let item = groups.iter().flat_map(|(_, i)| i).find(|i| i.id == id).expect("menu entry");
        assert!(!item.enabled, "{id}");
        assert_eq!(item.disabled_reason, Some(delete_dialog::UNC_REASON));
    }
    let err = r.app.run_command("file.delete").unwrap_err();
    assert!(err.contains("network"), "{err}");
    assert!(r.app.state().delete_dialog.is_none());
    // Even a hand-built dialog is refused by the engine with a friendly message.
    let session = r.app.session();
    let revision = session.revision();
    drop(session);
    let id = r.id_of("big.bin");
    r.app.state_mut().delete_dialog = Some(literal_dialog(id, revision, DeleteMode::Recycle));
    r.confirm();
    let error = r.dialog().error.expect("protected");
    assert!(error.message.contains("protected"), "{}", error.message);
    assert_eq!(error.hint, Hint::None);
    assert!(!r.fake.calls().iter().any(|c| matches!(c, PlatformCall::MoveToRecycleBin(_))));
    assert!(exists(&r.root().join("big.bin")));
    // The cleanups window disables Run with the same reason, and Run refuses.
    r.app.state_mut().delete_dialog = None;
    r.app.run_command("ui.cleanups").unwrap();
    r.frames(2, vec![]);
    let mut session = lock_session(&r.app.session);
    let err = cleanups_ui::open_run_dialog(&mut session, &mut r.app.state, "temporary-files").unwrap_err();
    assert_eq!(err, delete_dialog::UNC_REASON);
}

fn literal_dialog(id: usize, revision: u64, mode: DeleteMode) -> DeleteDialog {
    DeleteDialog {
        mode,
        subject: Subject::Node { id, revision },
        name: "x".into(),
        path: "x".into(),
        count: 1,
        files: 1,
        dirs: 0,
        size: 1,
        paths: vec![],
        truncated: false,
        partial_tree: false,
        skipped: 0,
        ack_permanent: true,
        ack_partial: true,
        error: None,
        focus_done: false,
    }
}

#[test]
fn enter_never_confirms_a_permanent_deletion() {
    assert!(delete_dialog::activation_allowed(DeleteMode::Recycle, true));
    assert!(!delete_dialog::activation_allowed(DeleteMode::Permanent, true));
    assert!(delete_dialog::activation_allowed(DeleteMode::Permanent, false));
    let mut r = Rig::new(0, None);
    let big = r.root().join("big.bin");
    let tab = || key(egui::Key::Tab, egui::Modifiers::NONE);
    let enter = || key(egui::Key::Enter, egui::Modifiers::NONE);
    // Permanent, fully acknowledged: Cancel has the focus, so Enter cancels.
    r.open("big.bin", "file.delete_permanent");
    {
        let d = r.app.state_mut().delete_dialog.as_mut().unwrap();
        d.ack_permanent = true;
    }
    r.frames(3, vec![]);
    assert!(r.dialog().focus_done);
    r.frames(1, vec![enter()]);
    r.frames(2, vec![]);
    assert!(r.app.state().delete_dialog.is_none() && exists(&big), "Enter on the default focus cancels");
    // Tab to the confirm button, then Enter: still nothing is deleted.
    r.open("big.bin", "file.delete_permanent");
    r.app.state_mut().delete_dialog.as_mut().unwrap().ack_permanent = true;
    r.frames(3, vec![]);
    // The widgets are created checkbox, Cancel, confirm: one Tab from Cancel reaches confirm.
    for _ in 0..1 {
        r.frames(1, vec![tab()]);
        r.frames(1, vec![]);
    }
    r.frames(1, vec![enter()]);
    r.frames(2, vec![]);
    assert!(exists(&big), "Enter never deletes permanently");
    let d = r.dialog();
    assert!(d.ack_permanent && d.error.is_none(), "the dialog is still open and untouched");
    assert!(r.fake.binned().is_empty());
}

#[test]
fn keyboard_can_confirm_a_recycle_only_after_leaving_the_default_focus() {
    let mut r = Rig::new(0, None);
    let big = r.root().join("big.bin");
    r.open("big.bin", "file.delete");
    r.frames(3, vec![]);
    // Enter on Cancel (the default focus) cancels.
    r.frames(1, vec![key(egui::Key::Enter, egui::Modifiers::NONE)]);
    r.frames(2, vec![]);
    assert!(r.app.state().delete_dialog.is_none() && exists(&big));
    // Tab moves to the confirm button; Enter then confirms the (reversible) recycle.
    r.open("big.bin", "file.delete");
    r.frames(3, vec![]);
    r.frames(1, vec![key(egui::Key::Tab, egui::Modifiers::NONE)]);
    r.frames(2, vec![]);
    r.frames(1, vec![key(egui::Key::Enter, egui::Modifiers::NONE)]);
    r.frames(2, vec![]);
    assert!(!exists(&big), "explicit keyboard activation of the confirm button");
    assert_eq!(r.fake.binned().len(), 1);
}

#[test]
fn delete_shortcuts_menus_and_requests_only_open_the_dialog() {
    let mut r = Rig::new(0, None);
    let big = r.root().join("big.bin");
    // Without a selection the shortcut explains itself.
    r.frames(1, vec![key(egui::Key::Delete, egui::Modifiers::NONE)]);
    assert!(r.app.state().delete_dialog.is_none());
    assert_eq!(r.app.state().notice.as_deref(), Some(SELECT_FIRST_MESSAGE));
    // With one, Delete and Shift+Delete open the two dialogs.
    r.select("big.bin");
    r.frames(1, vec![key(egui::Key::Delete, egui::Modifiers::NONE)]);
    assert_eq!(r.app.state().delete_dialog.as_ref().map(|d| d.mode), Some(DeleteMode::Recycle));
    r.frames(1, vec![key(egui::Key::Delete, egui::Modifiers::NONE)]);
    assert!(exists(&big), "a second Delete press behind the modal does nothing");
    r.app.state_mut().delete_dialog = None;
    r.frames(1, vec![key(egui::Key::Delete, egui::Modifiers::SHIFT)]);
    assert_eq!(r.app.state().delete_dialog.as_ref().map(|d| d.mode), Some(DeleteMode::Permanent));
    r.app.state_mut().delete_dialog = None;
    // ui.set_state requests open and close the dialog but cannot tick or confirm anything.
    assert!(r.app.apply_ui_patch(&json!({"delete_dialog": "recycle"})).is_ok());
    r.frames(2, vec![]);
    assert_eq!(r.app.state().delete_dialog.as_ref().map(|d| d.mode), Some(DeleteMode::Recycle));
    assert_eq!(r.app.ui_state_json()["delete_dialog"]["subject"]["type"], "node");
    for bad in [
        json!({"delete_dialog": "confirm"}),
        json!({"delete_dialog": {"ack_permanent": true}}),
        json!({"delete_dialog": null, "toast": {"kind": "info", "text": "x"}}),
    ] {
        assert!(r.app.apply_ui_patch(&bad).is_err(), "{bad}");
    }
    assert!(r.app.apply_ui_patch(&json!({"delete_dialog": "close"})).is_ok());
    r.frames(1, vec![]);
    assert!(r.app.state().delete_dialog.is_none() && exists(&big));
    // Menu entries: labels, shortcuts and enabling.
    let infos = r.app.session().commands();
    let groups = r.app.menu_groups(&infos);
    let find = |id: &str| {
        groups.iter().flat_map(|(_, i)| i).find(|i| i.id == id).map(|i| (i.label.clone(), i.shortcut, i.enabled))
    };
    assert_eq!(find("file.delete"), Some(("Move to Recycle Bin".to_owned(), Some("Delete"), true)));
    assert_eq!(find("file.delete_permanent"), Some(("Delete permanently...".to_owned(), Some("Shift+Delete"), true)));
    assert!(groups.iter().any(|(g, items)| g == "Tools" && items.iter().any(|i| i.id == "ui.cleanups")));
    assert!(groups.iter().any(|(g, items)| g == "Tools" && items.iter().any(|i| i.id == "ui.journal")));
    // The scan root cannot be deleted: the entries are off and say why.
    let root = r.app.session().tree().root().index();
    r.app.session().execute("select.node", &json!({ "id": root })).unwrap();
    let groups = r.app.menu_groups(&infos);
    let item = groups.iter().flat_map(|(_, i)| i).find(|i| i.id == "file.delete").unwrap();
    assert!(!item.enabled && item.disabled_reason == Some(delete_dialog::ROOT_REASON));
    assert!(r.app.run_command("file.delete").is_err());
}

#[test]
fn context_menu_entries_open_the_dialog_and_respect_the_block() {
    use crate::context_menu::{MenuAction, perform};
    let mut r = Rig::new(0, None);
    let node = r.app.session().tree().root();
    let big_id = r.id_of("big.bin");
    let big = r.app.session().tree().node_at(big_id).unwrap();
    let ctx = egui::Context::default();
    {
        let mut session = lock_session(&r.app.session);
        // The root is refused with a toast and no dialog.
        perform(&ctx, &mut session, &mut r.app.state, node, MenuAction::Recycle);
        assert!(r.app.state.delete_dialog.is_none() && r.app.state.toast.is_some());
        perform(&ctx, &mut session, &mut r.app.state, big, MenuAction::DeletePermanent);
    }
    assert_eq!(r.dialog().mode, DeleteMode::Permanent);
    assert!(exists(&r.root().join("big.bin")));
}

#[test]
fn cleanup_preview_dry_run_and_run_through_the_dialog() {
    let mut r = Rig::new(0, None);
    r.app.run_command("ui.cleanups").unwrap();
    r.frames(2, vec![]);
    assert_eq!(r.app.state().cleanups.entries.len(), 3);
    assert!(r.app.state().cleanups.entries.iter().all(|e| e.builtin));
    for theme in [ThemeKind::Dark, ThemeKind::Light] {
        r.app.state_mut().theme = theme;
        r.frames(2, vec![]);
    }
    let apply = |r: &mut Rig, a: CleanupsAction| {
        let mut session = lock_session(&r.app.session);
        DirustatApp::apply_cleanups_action(&mut r.app.state, &mut session, a);
    };
    apply(&mut r, CleanupsAction::Select("temporary-files".into()));
    apply(&mut r, CleanupsAction::Preview("temporary-files".into()));
    let view = r.app.state().cleanups.preview.clone().expect("preview");
    assert_eq!((view.count, view.ok, view.protected, view.stale, view.invalid), (2, 2, 0, 0, 0));
    r.frames(2, vec![]);
    // A dry run reports and changes nothing.
    apply(&mut r, CleanupsAction::DryRun("temporary-files".into()));
    let run = r.app.state().cleanups.last_run.clone().expect("dry run report");
    assert!(run.dry_run && run.deleted == 0 && run.count == 2);
    assert!(exists(&r.root().join("a.tmp")) && r.fake.binned().is_empty());
    r.frames(2, vec![]);
    // Run opens the dialog with the preview totals; cancelling leaves everything.
    apply(&mut r, CleanupsAction::Run("temporary-files".into()));
    let d = r.dialog();
    assert_eq!((d.mode, d.count, d.size, d.skipped), (DeleteMode::Recycle, 2, 740, 0));
    assert!(matches!(&d.subject, Subject::Cleanup { id } if id == "temporary-files"));
    assert_eq!(d.request().1["confirm"], true);
    for theme in [ThemeKind::Dark, ThemeKind::Light] {
        r.app.state_mut().theme = theme;
        r.frames(2, vec![]);
    }
    r.frames(1, vec![key(egui::Key::Escape, egui::Modifiers::NONE)]);
    r.frames(1, vec![]);
    assert!(r.app.state().delete_dialog.is_none() && exists(&r.root().join("a.tmp")));
    // Confirm: both files go to the fake bin and the report summarizes the run.
    apply(&mut r, CleanupsAction::Run("temporary-files".into()));
    r.confirm();
    assert_eq!(r.fake.binned().len(), 2);
    let run = r.app.state().cleanups.last_run.clone().unwrap();
    assert_eq!((run.deleted, run.failed, run.not_attempted, run.dry_run), (2, 0, 0, false));
    assert_eq!(run.stopped, None);
    assert!(r.app.state().toast.as_ref().is_some_and(|t| t.text.contains("Cleanup")));
    assert!(r.app.state().cleanups.preview.is_none(), "ids are invalid after a run");
    assert_eq!(r.journal_total(), 2);
    r.frames(2, vec![]);
    // A run on a tree with no matches opens no dialog.
    apply(&mut r, CleanupsAction::Run("temporary-files".into()));
    assert!(r.app.state().delete_dialog.is_none());
    assert!(r.app.state().toast.as_ref().is_some_and(|t| t.text.contains("Nothing to clean")));
}

#[test]
fn cleanups_preview_request_opens_the_window_and_only_previews() {
    let mut r = Rig::new(0, None);
    assert!(r.app.apply_ui_patch(&json!({"cleanups_preview": "temporary-files"})).is_ok());
    r.frames(3, vec![]);
    let st = r.app.state();
    assert!(st.cleanups.open);
    assert_eq!(st.cleanups.selected.as_deref(), Some("temporary-files"));
    assert_eq!(st.cleanups.preview.as_ref().map(|v| v.count), Some(2));
    assert!(st.delete_dialog.is_none() && st.cleanups.last_run.is_none());
    assert!(exists(&r.root().join("a.tmp")) && r.fake.binned().is_empty());
    // An oversized id is rejected by the strict patch (shared text limit).
    assert!(r.app.apply_ui_patch(&json!({"cleanups_preview": "x".repeat(crate::state::MAX_TEXT_LEN + 1)})).is_err());
}

#[test]
fn user_cleanups_are_edited_validated_and_a_permanent_one_needs_the_box() {
    let mut r = Rig::new(0, None);
    r.app.run_command("ui.cleanups").unwrap();
    r.frames(2, vec![]);
    let apply = |r: &mut Rig, a: CleanupsAction| {
        let mut session = lock_session(&r.app.session);
        DirustatApp::apply_cleanups_action(&mut r.app.state, &mut session, a);
    };
    // Built-ins are read-only: no form opens, removal is refused by the engine.
    apply(&mut r, CleanupsAction::Edit("temporary-files".into()));
    assert!(r.app.state().cleanups.form.is_none());
    apply(&mut r, CleanupsAction::Remove("temporary-files".into()));
    assert!(r.app.state().cleanups.error.is_some());
    // The form validates inline: nothing is saved with a bad size or no criterion.
    apply(&mut r, CleanupsAction::New);
    for theme in [ThemeKind::Dark, ThemeKind::Light] {
        r.app.state_mut().theme = theme;
        r.frames(2, vec![]);
    }
    let set = |r: &mut Rig, f: &dyn Fn(&mut CleanupForm)| f(r.app.state_mut().cleanups.form.as_mut().unwrap());
    set(&mut r, &|f| f.name = "Big bins".into());
    set(&mut r, &|f| f.min_size = "lots".into());
    apply(&mut r, CleanupsAction::Save);
    assert!(r.app.state().cleanups.form.as_ref().unwrap().error.as_deref().unwrap().contains("Minimum size"));
    set(&mut r, &|f| f.min_size.clear());
    apply(&mut r, CleanupsAction::Save);
    let engine_error = r.app.state().cleanups.form.as_ref().unwrap().error.clone().unwrap();
    assert!(engine_error.contains("criterion"), "{engine_error}");
    r.frames(2, vec![]);
    set(&mut r, &|f| {
        f.extensions = "bin".into();
        f.kind = KindChoice::File;
        f.action = ActionChoice::Permanent;
    });
    apply(&mut r, CleanupsAction::Save);
    assert!(r.app.state().cleanups.form.is_none());
    assert_eq!(r.app.state().cleanups.entries.len(), 4);
    assert_eq!(r.app.state().cleanups.selected.as_deref(), Some("big-bins"));
    assert_eq!(r.app.session().settings().cleanups.len(), 1);
    // Edit keeps the id; remove asks twice.
    apply(&mut r, CleanupsAction::Edit("big-bins".into()));
    assert_eq!(r.app.state().cleanups.form.as_ref().map(|f| f.extensions.clone()), Some("bin".to_owned()));
    apply(&mut r, CleanupsAction::CancelEdit);
    // The permanent cleanup: dialog with the red flow, two files, needs its box.
    apply(&mut r, CleanupsAction::Run("big-bins".into()));
    let d = r.dialog();
    assert_eq!((d.mode, d.count), (DeleteMode::Permanent, 2));
    r.confirm();
    assert!(exists(&r.root().join("big.bin")), "no box, no deletion");
    r.app.state_mut().delete_dialog.as_mut().unwrap().ack_permanent = true;
    let (cmd, params) = r.dialog().request();
    assert_eq!((cmd, &params["i_understand_permanent"]), ("cleanup.run", &json!(true)));
    r.confirm();
    assert!(!exists(&r.root().join("big.bin")) && !exists(&r.root().join("small.bin")));
    assert!(r.fake.binned().is_empty());
    apply(&mut r, CleanupsAction::AskRemove("big-bins".into()));
    assert_eq!(r.app.state().cleanups.confirm_remove.as_deref(), Some("big-bins"));
    r.frames(2, vec![]);
    apply(&mut r, CleanupsAction::Remove("big-bins".into()));
    assert_eq!(r.app.state().cleanups.entries.len(), 3);
    assert!(r.app.session().settings().cleanups.is_empty());
}

#[test]
fn journal_window_pages_through_the_entries() {
    let mut r = Rig::new(30, None);
    for i in 0..30 {
        let path = r.root().join(format!("f{i:02}.dat")).to_string_lossy().into_owned();
        r.app.session().execute("file.delete", &json!({ "path": path, "confirm": true })).unwrap();
    }
    assert!(r.app.run_command("ui.journal").is_ok());
    for theme in [ThemeKind::Dark, ThemeKind::Light] {
        r.app.state_mut().theme = theme;
        r.frames(2, vec![]);
    }
    let j = r.app.state().journal.clone();
    assert!(j.open && j.loaded);
    assert_eq!((j.total, j.offset, j.rows.len()), (30, 0, 25));
    assert!(j.rows.first().is_some_and(|row| row.path.ends_with("f29.dat") && row.ok && row.mode == "recycle"));
    r.app.state_mut().journal.turn(true);
    r.frames(2, vec![]);
    let j = r.app.state().journal.clone();
    assert_eq!((j.offset, j.rows.len()), (25, 5));
    assert!(j.rows.last().is_some_and(|row| row.path.ends_with("f00.dat")));
    r.app.state_mut().journal.turn(true);
    assert_eq!(r.app.state().journal.offset, 25, "the last page stays the last page");
    r.app.state_mut().journal.turn(false);
    r.frames(2, vec![]);
    assert_eq!(r.app.state().journal.offset, 0);
    // The window follows new actions: a confirmed delete marks it for reload.
    r.open("big.bin", "file.delete");
    r.confirm();
    assert!(!r.app.state().journal.loaded);
    r.frames(2, vec![]);
    assert_eq!(r.app.state().journal.total, 31);
    assert!(r.app.apply_ui_patch(&json!({"journal_open": false, "cleanups_open": true})).is_ok());
    assert!(!r.app.state().journal.open && r.app.state().cleanups.open);
}

#[test]
fn new_ui_state_round_trips_through_json() {
    let mut r = Rig::new(0, None);
    r.open("big.bin", "file.delete_permanent");
    r.app.state_mut().delete_dialog.as_mut().unwrap().ack_permanent = true;
    r.app.run_command("ui.cleanups").unwrap();
    r.app.run_command("ui.journal").unwrap();
    r.frames(3, vec![]);
    toast::push(&mut r.app.state, toast::ToastKind::Success, "done");
    cleanups_ui::build_definition(&CleanupForm { name: "x".into(), ..CleanupForm::default() }).unwrap();
    r.app.state_mut().cleanups.form = Some(CleanupForm { name: "n".into(), ..CleanupForm::default() });
    let state = r.app.state().clone();
    let json = serde_json::to_string(&state).unwrap();
    let back: UiState = serde_json::from_str(&json).unwrap();
    // Runtime-only fields (`#[serde(skip)]`) are not part of the serialized form.
    assert_eq!(serde_json::to_value(&back).unwrap(), serde_json::to_value(&state).unwrap());
    assert!(back.delete_dialog.as_ref().is_some_and(|d| d.ack_permanent));
    // A state saved before this feature still loads.
    let old: UiState = serde_json::from_str(
        "{\"path_text\":\"\",\"panels\":{\"tree\":true,\"map\":true,\"extensions\":true,\"status_bar\":true},\
         \"scan\":{\"state\":\"idle\"},\"theme\":\"dark\"}",
    )
    .unwrap();
    assert!(old.delete_dialog.is_none() && !old.cleanups.open && !old.journal.open && old.toast.is_none());
}
