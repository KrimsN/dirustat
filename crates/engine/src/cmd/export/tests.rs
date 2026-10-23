use super::*;
use crate::export::HARD_MAX_NODES;
use dirustat_model::{NodeData, NodeKind};
use std::path::Path;

/// Splits CSV text (RFC 4180) into records of cells; strict enough to catch quoting mistakes.
fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let (mut rows, mut row, mut cell) = (Vec::new(), Vec::new(), String::new());
    let mut chars = text.chars().peekable();
    let mut quoted = false;
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            (true, '"') => quoted = false,
            (true, c) => cell.push(c),
            (false, '"') => quoted = true,
            (false, ',') => row.push(std::mem::take(&mut cell)),
            (false, '\r') => {}
            (false, '\n') => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            (false, c) => cell.push(c),
        }
    }
    assert!(!quoted && cell.is_empty() && row.is_empty(), "unterminated record");
    rows
}

struct Fixture {
    dir: tempfile::TempDir,
    session: Session,
}

fn data(size: u64, mtime: Option<i64>) -> NodeData {
    NodeData { logical_size: size, allocated_size: size + 4096, mtime, ..NodeData::default() }
}

impl Fixture {
    /// Scan root `<tmp>/scan` (not on disk), output directory `<tmp>/out`. Nine nodes.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("out")).unwrap();
        let root = dir.path().join("scan").to_string_lossy().into_owned();
        let mut t = Tree::new(&root);
        let docs = t.add_child(t.root(), "Docs", NodeKind::Dir, 0).unwrap();
        t.add_node(docs, "Report.TXT", NodeKind::File, data(500, Some(1_700_000_000))).unwrap();
        t.add_node(docs, "=HYPERLINK(\"x\")", NodeKind::File, data(7, None)).unwrap();
        t.add_node(docs, "a,\"b\"\nc.md", NodeKind::File, data(3, Some(-5))).unwrap();
        let lossy = NodeData { name_lossy: true, ..data(1, None) };
        t.add_node(docs, "bad\u{fffd}name.bin", NodeKind::File, lossy).unwrap();
        t.add_node(t.root(), "big.bin", NodeKind::File, data(9000, Some(10))).unwrap();
        t.add_child(t.root(), "link", NodeKind::Link, 0).unwrap();
        t.add_child(t.root(), "empty", NodeKind::Dir, 0).unwrap();
        t.aggregate();
        let mut session = Session::new();
        session.replace_tree(t);
        Self { dir, session }
    }

    fn out(&self, name: &str) -> PathBuf {
        self.dir.path().join("out").join(name)
    }

    fn run(&mut self, id: &str, name: &str, extra: Value) -> Result<Value, CommandError> {
        let mut params = json!({ "path": self.out(name).to_string_lossy() });
        if let (Some(p), Some(e)) = (params.as_object_mut(), extra.as_object()) {
            p.extend(e.clone());
        }
        self.session.execute(id, &params)
    }
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> =
        std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

#[test]
fn csv_round_trips_with_quoting_and_formula_guard() {
    let mut f = Fixture::new();
    let out = f.run("export.csv", "all.csv", json!({})).unwrap();
    assert_eq!(out["rows"], 9);
    assert_eq!((out["truncated"].as_bool(), out["format"].as_str()), (Some(false), Some("csv")));
    let bytes = std::fs::read(f.out("all.csv")).unwrap();
    assert_eq!(out["bytes"].as_u64(), u64::try_from(bytes.len()).ok());
    assert!(!bytes.starts_with(b"\xEF\xBB\xBF"));
    let rows = parse_csv(&String::from_utf8(bytes).unwrap());
    assert_eq!(rows.len(), 10, "header + 9 nodes");
    assert_eq!(
        rows[0].join(","),
        "path,name,kind,depth,logical_size,allocated_size,file_count,mtime,extension,error,name_lossy"
    );
    assert!(rows.iter().all(|r| r.len() == 11), "every record has 11 cells");
    let by_name = |n: &str| rows.iter().find(|r| r[1] == n).unwrap_or_else(|| panic!("no row {n}"));
    let report = by_name("Report.TXT");
    assert_eq!(&report[2..], ["file", "2", "500", "4596", "1", "1700000000", "txt", "false", "false"]);
    assert!(report[0].ends_with("Report.TXT") && report[0].contains("Docs"));
    let docs = by_name("Docs");
    assert_eq!((docs[2].as_str(), docs[3].as_str(), docs[4].as_str(), docs[6].as_str()), ("dir", "1", "511", "4"));
    assert_eq!(docs[8], "", "directories have no extension");
    let weird = by_name("a,\"b\"\nc.md");
    assert_eq!((weird[7].as_str(), weird[8].as_str()), ("-5", "md"));
    assert!(weird[0].ends_with("a,\"b\"\nc.md"), "the comma/quote/newline name survives the quoting");
    assert_eq!(by_name("bad\u{fffd}name.bin")[10], "true");
    assert_eq!(by_name("link")[2], "link");
    assert_eq!(names_in(&f.dir.path().join("out")), ["all.csv"], "no temp file is left");
}

#[test]
fn csv_formula_names_get_a_quote_prefix_in_the_file() {
    let mut f = Fixture::new();
    f.run("export.csv", "f.csv", json!({"bom": true, "kind": "files"})).unwrap();
    let bytes = std::fs::read(f.out("f.csv")).unwrap();
    assert!(bytes.starts_with(b"\xEF\xBB\xBF"));
    let text = String::from_utf8(bytes[3..].to_vec()).unwrap();
    let rows = parse_csv(&text);
    let formula = rows.iter().find(|r| r[1].contains("HYPERLINK")).unwrap();
    assert_eq!(formula[1], "'=HYPERLINK(\"x\")");
    assert!(!formula[0].contains("'="), "the path starts with the scan root, not with a formula character");
    assert!(rows.iter().skip(1).all(|r| !r[1].starts_with(['=', '+', '-', '@'])));
}

#[test]
fn json_round_trips_through_serde() {
    let mut f = Fixture::new();
    let out = f.run("export.json", "all.json", json!({})).unwrap();
    assert_eq!(out["rows"], 9);
    let v: Value = serde_json::from_slice(&std::fs::read(f.out("all.json")).unwrap()).unwrap();
    assert_eq!(v["schema"], "dirustat.export.v1");
    assert_eq!((v["rows"].as_u64(), v["truncated"].as_bool()), (Some(9), Some(false)));
    assert!(v["truncated_reason"].is_null());
    let nodes = v["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 9);
    let node = |n: &str| nodes.iter().find(|x| x["name"] == n).unwrap_or_else(|| panic!("no node {n}"));
    assert_eq!(node("Report.TXT")["mtime"], 1_700_000_000_i64);
    assert!(node("=HYPERLINK(\"x\")")["mtime"].is_null(), "JSON keeps names exactly and unknown times as null");
    assert_eq!(node("a,\"b\"\nc.md")["extension"], "md");
    assert_eq!(node("bad\u{fffd}name.bin")["name_lossy"], true);
    assert_eq!((node("Docs")["file_count"].as_u64(), node("Docs")["logical_size"].as_u64()), (Some(4), Some(511)));
    assert_eq!(nodes[0]["depth"], 0);
    assert_eq!(nodes[0]["path"], v["scan_root"]);
}

#[test]
fn limits_subtree_depth_and_kind_select_rows() {
    let mut f = Fixture::new();
    let capped = f.run("export.csv", "n.csv", json!({"max_nodes": 3})).unwrap();
    assert_eq!((capped["rows"].as_u64(), capped["truncated_reason"].as_str()), (Some(3), Some("max_nodes")));
    assert_eq!(capped["truncated"], true);
    assert_eq!(parse_csv(&std::fs::read_to_string(f.out("n.csv")).unwrap()).len(), 4);
    // Exactly the number of nodes is not a truncation.
    assert_eq!(f.run("export.csv", "e.csv", json!({"max_nodes": 9})).unwrap()["truncated"], false);
    let rows = |f: &mut Fixture, name: &str, extra: Value| f.run("export.json", name, extra).unwrap()["rows"].as_u64();
    assert_eq!(rows(&mut f, "d0.json", json!({"max_depth": 0})), Some(1));
    assert_eq!(rows(&mut f, "d1.json", json!({"max_depth": 1})), Some(5));
    assert_eq!(rows(&mut f, "files.json", json!({"kind": "files"})), Some(6));
    assert_eq!(rows(&mut f, "dirs.json", json!({"kind": "dirs"})), Some(3));
    let docs = f.session.tree().children(f.session.tree().root()).next().unwrap();
    assert_eq!(rows(&mut f, "sub.json", json!({"node": docs.index()})), Some(5));
    let v: Value = serde_json::from_slice(&std::fs::read(f.out("sub.json")).unwrap()).unwrap();
    assert_eq!((v["nodes"][0]["name"].as_str(), v["nodes"][0]["depth"].as_u64()), (Some("Docs"), Some(0)));
    assert!(v["root"].as_str().unwrap().ends_with("Docs"));
    // A byte budget stops after the row that reaches it and still writes valid JSON.
    let tiny = f.run("export.json", "tiny.json", json!({"max_bytes": 1})).unwrap();
    assert_eq!((tiny["truncated_reason"].as_str(), tiny["rows"].as_u64()), (Some("max_bytes"), Some(1)));
    let v: Value = serde_json::from_slice(&std::fs::read(f.out("tiny.json")).unwrap()).unwrap();
    assert_eq!(v["nodes"].as_array().unwrap().len(), 1);
    for bad in [
        json!({"max_nodes": 0}),
        json!({"max_nodes": HARD_MAX_NODES + 1}),
        json!({"max_bytes": 0}),
        json!({"node": 99999}),
        json!({"kind": "x"}),
        json!({"max_depth": -1}),
        json!({"bom": true}),
        json!({"nope": 1}),
    ] {
        let err = f.run("export.json", "bad.json", bad.clone()).unwrap_err();
        assert_eq!(err.code(), "invalid_params", "{bad}");
    }
    assert!(!f.out("bad.json").exists());
}

/// QA-152: `node` may carry the revision it was read at; a stale one is `stale_tree`.
#[test]
fn node_with_a_stale_revision_is_refused_and_nothing_is_written() {
    let mut f = Fixture::new();
    let docs = f.session.tree().children(f.session.tree().root()).next().unwrap().index();
    let rev = f.session.revision();
    for (id, name) in [("export.csv", "a.csv"), ("export.json", "a.json")] {
        let ok = f.run(id, name, json!({"node": docs, "revision": rev})).unwrap();
        assert_eq!(ok["rows"], 5, "matching revision exports the subtree");
        // Without a revision the call keeps working.
        let bare = f.run(id, &format!("bare-{name}"), json!({"node": docs})).unwrap();
        assert_eq!(bare["rows"], 5);
        let stale = f.run(id, &format!("stale-{name}"), json!({"node": docs, "revision": rev + 1})).unwrap_err();
        assert_eq!(stale.code(), "stale_tree", "{stale}");
        assert!(!f.out(&format!("stale-{name}")).exists());
    }
    // The tree changes: the old revision no longer matches, even without `node`.
    let mut t = Tree::new(f.session.tree().get(f.session.tree().root()).unwrap().name());
    t.add_node(t.root(), "only.bin", NodeKind::File, data(1, None)).unwrap();
    t.aggregate();
    f.session.replace_tree(t);
    for params in [json!({"node": docs, "revision": rev}), json!({"revision": rev})] {
        let err = f.run("export.csv", "after.csv", params).unwrap_err();
        assert_eq!(err.code(), "stale_tree");
    }
    assert!(!f.out("after.csv").exists());
    let now = f.session.revision();
    assert_eq!(f.run("export.csv", "now.csv", json!({"revision": now})).unwrap()["rows"], 2);
}

#[test]
fn deep_chains_stream_without_recursion() {
    let mut t = Tree::new("r");
    let mut cur = t.root();
    for _ in 0..3000 {
        cur = t.add_child(cur, "d", NodeKind::Dir, 0).unwrap();
    }
    t.aggregate();
    let sel = Selection {
        root: t.root(),
        max_depth: None,
        rows: RowKind::All,
        max_nodes: u64::MAX,
        max_bytes: u64::MAX,
        bom: false,
    };
    let mut out = Vec::new();
    let summary = export::write_table(&mut out, &t, Format::Csv, &sel).unwrap();
    assert_eq!(summary.rows, 3001);
    let text = String::from_utf8(out).unwrap();
    let path = text.lines().last().unwrap().split(',').next().unwrap();
    let sep = if cfg!(windows) { '\\' } else { '/' };
    assert_eq!(path.split(sep).count(), 3001);
}

#[test]
fn existing_files_are_refused_unless_overwrite_and_only_plain_files_are_replaced() {
    let mut f = Fixture::new();
    f.run("export.csv", "x.csv", json!({})).unwrap();
    let first = std::fs::read(f.out("x.csv")).unwrap();
    let err = f.run("export.csv", "x.csv", json!({"max_nodes": 2})).unwrap_err();
    assert_eq!(err.code(), "invalid_params");
    assert!(err.to_string().contains("overwrite"), "{err}");
    assert_eq!(std::fs::read(f.out("x.csv")).unwrap(), first, "the file is untouched");
    f.run("export.csv", "x.csv", json!({"max_nodes": 2, "overwrite": true})).unwrap();
    assert_ne!(std::fs::read(f.out("x.csv")).unwrap(), first);
    // A directory with the target name is never replaced.
    std::fs::create_dir(f.out("dir.csv")).unwrap();
    let err = f.run("export.csv", "dir.csv", json!({"overwrite": true})).unwrap_err();
    assert_eq!(err.code(), "invalid_params");
    assert_eq!(names_in(&f.dir.path().join("out")), ["dir.csv", "x.csv"]);
}

#[test]
fn a_file_that_appears_during_the_write_is_not_clobbered() {
    let f = Fixture::new();
    let target = f.out("race.csv");
    let platform = f.session.platform();
    let result = export::write_atomic(&target, false, platform.as_ref(), |file| {
        use std::io::Write;
        file.write_all(b"new")?;
        std::fs::write(&target, b"someone else")?;
        Ok(())
    });
    assert_eq!(result.unwrap_err().code(), "invalid_params");
    assert_eq!(std::fs::read(&target).unwrap(), b"someone else");
    assert_eq!(names_in(&f.dir.path().join("out")), ["race.csv"], "the temp file is removed");
}

#[test]
fn bad_targets_are_refused_without_touching_the_disk() {
    let mut f = Fixture::new();
    let inside = f.dir.path().join("scan");
    std::fs::create_dir(&inside).unwrap();
    let missing_dir = f.dir.path().join("nope").join("x.csv");
    let cases = [
        (String::new(), "empty"),
        ("a\0b.csv".to_owned(), "NUL"),
        (f.out("x.txt").to_string_lossy().into_owned(), "extension"),
        (f.out("x.json").to_string_lossy().into_owned(), "other format's extension"),
        (f.out("noext").to_string_lossy().into_owned(), "no extension"),
        (missing_dir.to_string_lossy().into_owned(), "missing directory"),
        ("\\\\server\\share\\x.csv".to_owned(), "UNC"),
        ("//server/share/x.csv".to_owned(), "UNC slashes"),
        ("\\\\?\\GLOBALROOT\\Device\\x.csv".to_owned(), "device path"),
        ("x".repeat(40_000) + ".csv", "too long"),
        (inside.join("x.csv").to_string_lossy().into_owned(), "inside the scan"),
        (inside.join("..").join("scan").join("x.csv").to_string_lossy().into_owned(), "inside the scan via .."),
    ];
    for (path, what) in cases {
        let err = f.session.execute("export.csv", &json!({ "path": path })).unwrap_err();
        assert_eq!(err.code(), "invalid_params", "{what}: {err}");
    }
    assert!(!missing_dir.parent().unwrap().exists() && names_in(&inside).is_empty());
    if cfg!(windows) {
        for name in ["CON.csv", "a:b.csv", "x?.csv"] {
            let err = f.run("export.csv", name, json!({})).unwrap_err();
            assert_eq!(err.code(), "invalid_params", "{name}");
        }
    }
    // The scanned folder is writable only on request.
    let target = inside.join("in.csv");
    f.session.execute("export.csv", &json!({"path": target.to_string_lossy(), "allow_inside_scan": true})).unwrap();
    assert!(target.is_file());
}

#[test]
fn png_needs_the_desktop_app_and_validates_its_params() {
    let mut f = Fixture::new();
    // No renderer is registered in the engine's own tests.
    assert_eq!(f.run("export.png", "t.png", json!({})).unwrap_err().code(), "unsupported");
    for bad in [json!({"width": 0}), json!({"theme": "blue"}), json!({"x": 1})] {
        let err = f.run("export.png", "t.png", bad.clone()).unwrap_err();
        assert!(matches!(err.code(), "invalid_params" | "unsupported"), "{bad}: {err}");
    }
    assert!(!f.out("t.png").exists());
}

#[test]
fn export_is_a_detached_job_that_runs_without_the_session() {
    let mut f = Fixture::new();
    let params = json!({"path": f.out("job.json").to_string_lossy()});
    let crate::Prepared::Job(job) = f.session.execute_split("export.json", &params) else {
        panic!("export.json should be split off")
    };
    assert!(!f.out("job.json").exists(), "nothing is written under the lock");
    // The session stays usable, and replacing the tree does not change the snapshot.
    f.session.replace_tree(Tree::new("other"));
    let outcome = std::thread::spawn(move || job.run()).join().unwrap();
    let crate::Prepared::Done(done) = f.session.finish_job(outcome) else { panic!("one step only") };
    assert_eq!(done.unwrap()["rows"], 9);
    assert!(f.out("job.json").is_file());
    // An empty session cannot export.
    assert_eq!(f.session.execute("export.json", &params).unwrap_err().code(), "not_enabled");
}
