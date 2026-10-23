use super::*;
use crate::cmd::registry;
use crate::snapshot::tests::{dump, sample};
use crate::{Prepared, execute_unlocked};
use std::sync::Mutex;

struct Fixture {
    dir: tempfile::TempDir,
    session: Session,
}

impl Fixture {
    /// Scan root `<tmp>/scan` (not on disk) holding the sample tree, output directory `<tmp>/out`.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("out")).unwrap();
        let mut t = sample();
        t.set_node_name(t.root(), &dir.path().join("scan").to_string_lossy()).unwrap();
        let mut session = Session::new();
        session.replace_tree(t);
        Self { dir, session }
    }

    fn out(&self, name: &str) -> PathBuf {
        self.dir.path().join("out").join(name)
    }

    fn save(&mut self, name: &str) -> Result<Value, CommandError> {
        self.session.execute("snapshot.save", &json!({ "path": self.out(name).to_string_lossy() }))
    }

    fn load(&mut self, name: &str, extra: Value) -> Result<Value, CommandError> {
        let mut params = json!({ "path": self.out(name).to_string_lossy() });
        if let (Some(p), Some(e)) = (params.as_object_mut(), extra.as_object()) {
            p.extend(e.clone());
        }
        self.session.execute("snapshot.load", &params)
    }
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> =
        std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

#[test]
fn save_and_load_round_trip_into_a_flagged_view() {
    let mut f = Fixture::new();
    let before = dump(f.session.tree());
    let saved = f.save("a.dirsnap").unwrap();
    assert_eq!(saved["nodes"], 8);
    assert_eq!(saved["bytes"].as_u64(), Some(std::fs::metadata(f.out("a.dirsnap")).unwrap().len()));
    assert_eq!(names_in(&f.dir.path().join("out")), ["a.dirsnap"], "no temp file is left behind");
    assert!(f.session.snapshot_view().is_none());
    assert!(f.session.execute("session.state", &json!({})).unwrap()["snapshot"].is_null());

    // Another tree is current; loading brings the saved one back as a view.
    f.session.replace_tree(dirustat_scan::empty_tree("other"));
    let loaded = f.load("a.dirsnap", json!({})).unwrap();
    assert_eq!((loaded["showing"].as_bool(), loaded["nodes"].as_u64()), (Some(true), Some(8)));
    assert_eq!(dump(f.session.tree()), before);
    let state = f.session.execute("session.state", &json!({})).unwrap();
    assert_eq!(state["snapshot"]["id"], loaded["id"]);
    assert_eq!(state["nodes"], 8);
    let list = f.session.execute("snapshot.list", &json!({})).unwrap();
    assert_eq!(list["viewing"], loaded["id"]);
    assert_eq!(list["snapshots"][0]["viewing"], true);
    // The view is browsable like a scan.
    let kids = f.session.execute("tree.children", &json!({})).unwrap();
    assert_eq!(kids["total"], 4);
}

#[test]
fn save_refuses_overwrite_and_bad_targets() {
    let mut f = Fixture::new();
    f.save("a.dirsnap").unwrap();
    let err = f.save("a.dirsnap").unwrap_err();
    assert_eq!(err.code(), "invalid_params");
    let params = json!({ "path": f.out("a.dirsnap").to_string_lossy(), "overwrite": true });
    f.session.execute("snapshot.save", &params).unwrap();
    for bad in [
        json!({ "path": f.out("a.txt").to_string_lossy() }),
        json!({ "path": f.dir.path().join("missing").join("a.dirsnap").to_string_lossy() }),
        json!({ "path": f.dir.path().join("scan").join("a.dirsnap").to_string_lossy() }),
        json!({ "path": "" }),
        json!({ "path": f.out("b.dirsnap").to_string_lossy(), "extra": 1 }),
    ] {
        assert_eq!(f.session.execute("snapshot.save", &bad).unwrap_err().code(), "invalid_params", "{bad}");
    }
    assert_eq!(names_in(&f.dir.path().join("out")), ["a.dirsnap"]);
}

#[test]
fn corrupt_files_fail_to_load_and_change_nothing() {
    let mut f = Fixture::new();
    f.save("a.dirsnap").unwrap();
    let good = std::fs::read(f.out("a.dirsnap")).unwrap();
    let revision = f.session.revision();
    let write = |f: &Fixture, name: &str, bytes: &[u8]| std::fs::write(f.out(name), bytes).unwrap();
    let mut flipped = good.clone();
    let mid = flipped.len() / 2;
    flipped[mid] ^= 0x55;
    write(&f, "flipped.dirsnap", &flipped);
    write(&f, "cut.dirsnap", &good[..good.len() - 10]);
    write(&f, "empty.dirsnap", b"");
    write(&f, "text.dirsnap", b"this is not a snapshot at all, just some text that is long enough to pass");
    let mut newer = good;
    newer[8] = 9;
    write(&f, "newer.dirsnap", &newer);
    for name in ["flipped.dirsnap", "cut.dirsnap", "empty.dirsnap", "text.dirsnap", "newer.dirsnap"] {
        let err = f.load(name, json!({})).unwrap_err();
        assert_eq!(err.code(), "invalid_params", "{name}: {err}");
    }
    assert!(f.load("nope.dirsnap", json!({})).is_err());
    let dir_as_file = json!({ "path": f.dir.path().join("out").to_string_lossy() });
    assert!(f.session.execute("snapshot.load", &dir_as_file).is_err());
    assert_eq!(f.session.revision(), revision, "the tree was not touched");
    assert!(f.session.snapshot_view().is_none());
    assert_eq!(f.session.execute("snapshot.list", &json!({})).unwrap()["snapshots"], json!([]));
    // A node limit below the file's count is refused.
    assert!(f.load("a.dirsnap", json!({ "max_nodes": 3 })).is_err());
    assert!(f.load("a.dirsnap", json!({ "max_nodes": 0 })).is_err());
}

#[test]
fn destructive_commands_are_refused_on_a_snapshot_view() {
    let mut f = Fixture::new();
    f.save("a.dirsnap").unwrap();
    f.load("a.dirsnap", json!({})).unwrap();
    let destructive: Vec<&str> = registry().iter().filter(|s| s.destructive).map(|s| s.id).collect();
    assert!(destructive.len() >= 4, "{destructive:?}");
    for id in destructive {
        for params in [json!({}), json!({ "confirm": true }), json!({ "confirm": true, "id": 1, "revision": 1 })] {
            let err = f.session.execute(id, &params).unwrap_err();
            assert_eq!(err.code(), "not_enabled", "{id} {params}: {err}");
        }
    }
    // The split entry point (used by the UI and the control server) refuses as well.
    match f.session.execute_split("file.delete_permanent", &json!({ "confirm": true })) {
        Prepared::Done(Err(e)) => assert_eq!(e.code(), "not_enabled"),
        other => panic!("{other:?}"),
    }
    assert_eq!(f.session.tree().len(), 8);
}

#[test]
fn the_flag_ends_with_a_new_tree_or_close() {
    let mut f = Fixture::new();
    f.save("a.dirsnap").unwrap();
    let id = f.load("a.dirsnap", json!({})).unwrap()["id"].as_u64().unwrap();
    f.session.replace_tree(dirustat_scan::empty_tree("x"));
    assert!(f.session.snapshot_view().is_none(), "a replaced tree is no longer the snapshot");
    // The snapshot stays loaded for comparisons.
    assert_eq!(f.session.execute("snapshot.list", &json!({})).unwrap()["snapshots"][0]["viewing"], false);

    let again = f.load("a.dirsnap", json!({})).unwrap()["id"].as_u64().unwrap();
    assert_ne!(again, id);
    assert!(f.session.snapshot_view().is_some());
    let closed = f.session.execute("snapshot.close", &json!({ "id": again })).unwrap();
    assert_eq!(closed["was_viewing"], true);
    assert!(f.session.snapshot_view().is_none());
    assert_eq!(f.session.tree().len(), 1, "back to an empty tree");
    assert_eq!(f.session.execute("snapshot.close", &json!({ "id": again })).unwrap_err().code(), "invalid_params");
    f.session.execute("snapshot.close", &json!({ "id": id })).unwrap();
    assert_eq!(f.session.execute("snapshot.list", &json!({})).unwrap()["snapshots"], json!([]));
}

#[test]
fn at_most_four_snapshots_are_held() {
    let mut f = Fixture::new();
    f.save("a.dirsnap").unwrap();
    for _ in 0..MAX_LOADED {
        f.load("a.dirsnap", json!({ "show": false })).unwrap();
    }
    assert!(f.session.snapshot_view().is_none(), "show: false keeps the current tree");
    let err = f.load("a.dirsnap", json!({ "show": false })).unwrap_err();
    assert_eq!(err.code(), "invalid_params");
}

#[test]
fn compare_live_with_a_loaded_snapshot() {
    let mut f = Fixture::new();
    f.save("a.dirsnap").unwrap();
    let id = f.load("a.dirsnap", json!({ "show": false })).unwrap()["id"].clone();
    // Identical right after saving.
    let same = f.session.execute("snapshot.compare", &json!({ "base": id })).unwrap();
    assert_eq!(same["identical"], true);
    assert_eq!(same["changes"], json!([]));
    assert_eq!(same["base"]["source"], "snapshot");
    assert_eq!(same["other"]["source"], "live");

    // Change the live tree: a grown file, a removed one, a new directory.
    let mut t = Tree::new(&scan_name(&f));
    let docs = t.add_child(t.root(), "docs", NodeKind::Dir, 0).unwrap();
    t.add_child(docs, "a.txt", NodeKind::File, 4100).unwrap();
    let n = t.add_child(t.root(), "new", NodeKind::Dir, 0).unwrap();
    t.add_child(n, "z.bin", NodeKind::File, 77).unwrap();
    t.aggregate();
    f.session.replace_tree(t);
    let params = json!({ "base": id, "size_mode": "logical", "limit": 10 });
    let diff = f.session.execute("snapshot.compare", &params).unwrap();
    assert_eq!(diff["identical"], false);
    let changes = diff["changes"].as_array().unwrap();
    let find = |kind: &str| changes.iter().find(|c| c["kind"] == kind).unwrap_or_else(|| panic!("no {kind}"));
    assert_eq!(find("grown")["path"], "docs/a.txt");
    assert_eq!(find("grown")["delta"], 4000);
    assert_eq!(find("added")["path"], "new");
    assert_eq!(find("added")["new_id"].as_u64().map(|i| f.session.snapshot_node_check(i)), Some(true));
    assert!(changes.iter().any(|c| c["kind"] == "removed" && c["path"] == "media"));
    assert_eq!(diff["counts"]["added"], 1);
    assert_eq!(diff["truncated"], false);
    assert_eq!(
        diff["delta"].as_i64(),
        Some(
            i64::try_from(diff["other_total"].as_u64().unwrap()).unwrap()
                - i64::try_from(diff["base_total"].as_u64().unwrap()).unwrap()
        )
    );

    let limited =
        f.session.execute("snapshot.compare", &json!({ "base": id, "limit": 1, "size_mode": "logical" })).unwrap();
    assert_eq!((limited["changes"].as_array().unwrap().len(), limited["truncated"].as_bool()), (1, Some(true)));
    let docs_only =
        f.session.execute("snapshot.compare", &json!({ "base": id, "prefix": "docs\\", "kinds": ["grown"] })).unwrap();
    assert_eq!(docs_only["changes"].as_array().unwrap().len(), 1);
    assert_eq!(docs_only["prefix"], "docs");
}

fn scan_name(f: &Fixture) -> String {
    f.session.tree().get(f.session.tree().root()).unwrap().name().to_owned()
}

impl Session {
    fn snapshot_node_check(&self, index: u64) -> bool {
        self.tree().node_at(usize::try_from(index).unwrap()).is_some()
    }
}

#[test]
fn compare_params_are_validated() {
    let mut f = Fixture::new();
    for bad in [
        json!({}),
        json!({ "base": "yesterday" }),
        json!({ "base": 99 }),
        json!({ "base": "live", "other": 99 }),
        json!({ "base": "live", "limit": 0 }),
        json!({ "base": "live", "limit": 10001 }),
        json!({ "base": "live", "kinds": ["moved"] }),
        json!({ "base": "live", "prefix": "a\0b" }),
        json!({ "base": "live", "bogus": 1 }),
    ] {
        assert_eq!(f.session.execute("snapshot.compare", &bad).unwrap_err().code(), "invalid_params", "{bad}");
    }
    let same = f.session.execute("snapshot.compare", &json!({ "base": "live" })).unwrap();
    assert_eq!(same["identical"], true);
}

#[test]
fn split_execution_runs_the_slow_part_without_the_session() {
    let mut f = Fixture::new();
    let path = f.out("s.dirsnap").to_string_lossy().into_owned();
    let params = json!({ "path": path });
    let Prepared::Job(job) = f.session.execute_split("snapshot.save", &params) else { panic!("expected a job") };
    let outcome = job.run();
    let Prepared::Done(done) = f.session.finish_job(outcome) else { panic!("expected a result") };
    assert_eq!(done.unwrap()["nodes"], 8);

    let shared = Mutex::new(f.session);
    let loaded = execute_unlocked(|| shared.lock().unwrap(), "snapshot.load", &params).unwrap();
    assert_eq!(loaded["showing"], true);
    assert!(shared.lock().unwrap().snapshot_view().is_some());
}

#[test]
fn a_snapshot_of_a_snapshot_view_can_be_saved_again() {
    let mut f = Fixture::new();
    f.save("a.dirsnap").unwrap();
    f.load("a.dirsnap", json!({})).unwrap();
    f.save("b.dirsnap").unwrap();
    let (a, b) = (std::fs::read(f.out("a.dirsnap")).unwrap(), std::fs::read(f.out("b.dirsnap")).unwrap());
    assert_eq!(a.len(), b.len());
}

// ------------------------------------------------------------------------------ M5 review fixes

/// Writes a snapshot file whose root is named `root` (bypassing `snapshot.save`'s checks).
fn write_with_root(f: &Fixture, name: &str, root: &str) {
    let mut t = sample();
    t.set_node_name(t.root(), root).unwrap();
    let file = std::fs::File::create(f.out(name)).unwrap();
    format::write(file, &t, false, 0).unwrap();
}

#[test]
fn a_hostile_root_is_refused_at_load() {
    let mut f = Fixture::new();
    for (i, root) in [r"\host\share", "//host/share", r"\?\UNC\host\share", r"\.\PhysicalDrive0", "relative", ""]
        .into_iter()
        .enumerate()
    {
        let name = format!("h{i}.dirsnap");
        write_with_root(&f, &name, root);
        let err = f.load(&name, json!({})).unwrap_err();
        assert_eq!(err.code(), "invalid_params", "{root:?}: {err}");
    }
    assert!(f.session.snapshot_view().is_none());
    assert_eq!(f.session.execute("snapshot.list", &json!({})).unwrap()["snapshots"], json!([]));
}

#[test]
fn id_derived_paths_are_refused_under_a_snapshot_view() {
    let mut f = Fixture::new();
    f.save("a.dirsnap").unwrap();
    f.load("a.dirsnap", json!({})).unwrap();
    for (id, params) in [
        ("platform.reveal", json!({ "id": 1 })),
        ("platform.properties", json!({ "id": 1 })),
        ("duplicates.find", json!({})),
    ] {
        let err = f.session.execute(id, &params).unwrap_err();
        assert_eq!(err.code(), "not_enabled", "{id}: {err}");
        assert!(err.to_string().contains("snapshot"), "{id}: {err}");
    }
}

#[test]
fn compare_with_live_is_refused_while_a_snapshot_is_shown() {
    let mut f = Fixture::new();
    f.save("a.dirsnap").unwrap();
    let id = f.load("a.dirsnap", json!({ "show": false })).unwrap()["id"].as_u64().unwrap();
    assert!(f.session.execute("snapshot.compare", &json!({ "base": id })).is_ok(), "fine while live is live");
    f.load("a.dirsnap", json!({})).unwrap();
    for params in
        [json!({ "base": id }), json!({ "base": "live", "other": id }), json!({ "base": id, "other": "live" })]
    {
        let err = f.session.execute("snapshot.compare", &params).unwrap_err();
        assert_eq!(err.code(), "invalid_params", "{params}: {err}");
        assert!(err.to_string().contains("live"), "{err}");
    }
    let two = f.session.execute("snapshot.list", &json!({})).unwrap()["snapshots"][0]["id"].as_u64().unwrap();
    assert!(f.session.execute("snapshot.compare", &json!({ "base": id, "other": two })).is_ok());
}

#[test]
fn loads_in_flight_count_against_the_cap() {
    let mut f = Fixture::new();
    f.save("a.dirsnap").unwrap();
    let params = json!({ "path": f.out("a.dirsnap").to_string_lossy(), "show": false });
    let mut jobs = Vec::new();
    for _ in 0..MAX_LOADED {
        match f.session.execute_split("snapshot.load", &params) {
            Prepared::Job(job) => jobs.push(job),
            other => panic!("{other:?}"),
        }
    }
    match f.session.execute_split("snapshot.load", &params) {
        Prepared::Done(Err(e)) => assert_eq!(e.code(), "invalid_params", "{e}"),
        other => panic!("a load beyond the cap must be refused at begin: {other:?}"),
    }
    // Finishing the jobs stores them; none was lost or double counted.
    for job in jobs {
        let out = job.run();
        match f.session.finish_job(out) {
            Prepared::Done(Ok(_)) => {}
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(
        f.session.execute("snapshot.list", &json!({})).unwrap()["snapshots"].as_array().unwrap().len(),
        MAX_LOADED
    );
    // Dropped (never run) jobs free their slots again.
    f.session.execute("snapshot.close", &json!({ "id": 1 })).unwrap();
    match f.session.execute_split("snapshot.load", &params) {
        Prepared::Job(job) => drop(job),
        other => panic!("{other:?}"),
    }
    assert!(f.load("a.dirsnap", json!({ "show": false })).is_ok());
}
