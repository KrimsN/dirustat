//! Commands whose slow part runs **without the session lock** (QA-143, QA-144).
//!
//! A host that shares the session behind a mutex (the control server, MCP, the UI) calls
//! [`Session::execute_split`] under the lock. For `platform.free_space`, `file.delete_permanent`
//! and `cleanup.run` it returns a [`DetachedJob`] instead of a result: everything that needs the
//! session (parameters, every safety check, the plan) is already done, and the job owns what the
//! slow part needs. The host releases the lock, calls [`DetachedJob::run`], re-locks and hands the
//! outcome to [`Session::finish_job`], which journals and prunes the tree. A cleanup run is a chain
//! of such jobs (one per action, G-062): `finish_job` hands back the next one until the run is over.
//! Any other command is simply run. [`execute_unlocked`] is that loop for hosts with a lock function.
//!
//! While a permanent deletion or a cleanup run is in flight the session refuses destructive
//! commands and `scan.*` (`not_enabled`), so the plan cannot be invalidated by a second deleter;
//! every other command (state, listings, search) keeps working on the not-yet-pruned tree. If the
//! tree was replaced anyway (a host called `replace_tree` or `finish_scan` directly) the outcome is
//! journaled and the session is flagged `rescan_recommended` instead of pruning the wrong tree.
//!
//! `Session::execute` keeps the all-in-one behaviour for single-threaded callers (CLI, tests).

use super::cleanup::{self, CleanupStep, Next, StepOutcome};
use super::file::{self, Begun, RemovalJob, RemovalOutcome};
use super::platform::{self, FreeSpaceJob};
use super::{admit, registry};
use crate::{CommandError, Session, guard};
use serde_json::Value;
use std::ops::DerefMut;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// What [`Session::execute_split`] gives back.
#[derive(Debug)]
pub enum Prepared {
    /// The command finished under the lock (including every error).
    Done(Result<Value, CommandError>),
    /// The slow part is left to the caller: run it without the lock.
    Job(DetachedJob),
}

/// The slow part of a command. `Send + 'static`: it holds no reference to the session.
#[derive(Debug)]
pub struct DetachedJob(Kind);

#[derive(Debug)]
enum Kind {
    FreeSpace(FreeSpaceJob),
    Removal(RemovalJob),
    Cleanup(CleanupStep),
}

/// The result of [`DetachedJob::run`], to be passed to [`Session::finish_job`].
#[derive(Debug)]
pub struct JobOutcome(Done);

#[derive(Debug)]
enum Done {
    Value(Result<Value, CommandError>),
    Removal(RemovalOutcome),
    Cleanup(StepOutcome),
}

impl DetachedJob {
    /// Does the slow work. Needs no session; never panics (a panic becomes an error).
    pub fn run(self) -> JobOutcome {
        JobOutcome(match self.0 {
            Kind::FreeSpace(job) => Done::Value(job.run()),
            Kind::Removal(job) => Done::Removal(job.run()),
            Kind::Cleanup(step) => Done::Cleanup(step.run()),
        })
    }
}

/// Runs `id` to the end through [`Session::execute_split`]: `lock` is called for every part that
/// needs the session and the guard is dropped before each slow part. Blocks until the command is
/// done, so a UI calls it from a worker thread.
pub fn execute_unlocked<G: DerefMut<Target = Session>>(
    lock: impl Fn() -> G,
    id: &str,
    params: &Value,
) -> Result<Value, CommandError> {
    let mut prepared = lock().execute_split(id, params);
    loop {
        match prepared {
            Prepared::Done(result) => return result,
            Prepared::Job(job) => {
                let outcome = job.run();
                prepared = lock().finish_job(outcome);
            }
        }
    }
}

/// A flag held for as long as a slow operation is in flight. Dropping it, also while unwinding
/// from a panic, clears the flag, so a crashed helper cannot block the feature for good.
#[derive(Debug)]
pub(crate) struct BusyGuard(Arc<AtomicBool>);

impl BusyGuard {
    /// Takes the flag; `None` when somebody else holds it.
    pub(crate) fn claim(flag: &Arc<AtomicBool>) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).ok().map(|_| Self(Arc::clone(flag)))
    }
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl Session {
    /// Like [`Session::execute`], but the slow part of `platform.free_space`,
    /// `file.delete_permanent` and `cleanup.run` is returned as a [`DetachedJob`] for the caller to
    /// run without holding the session. All other commands are executed here.
    pub fn execute_split(&mut self, id: &str, params: &Value) -> Prepared {
        let begun = match id {
            "platform.free_space" => begin(self, id, params, |s, p| {
                platform::begin_free_space(s, p).map(|job| Ok(DetachedJob(Kind::FreeSpace(job))))
            }),
            "file.delete_permanent" => begin(self, id, params, |s, p| {
                file::begin_permanent(s, p).map(|b| match b {
                    Begun::Report(report) => Err(report),
                    Begun::Job(job) => Ok(DetachedJob(Kind::Removal(job))),
                })
            }),
            "cleanup.run" => begin(self, id, params, |s, p| cleanup::begin_run(s, p).map(cleanup_job)),
            _ => return Prepared::Done(self.execute(id, params)),
        };
        match begun {
            Ok(Ok(job)) => Prepared::Job(job),
            Ok(Err(report)) => Prepared::Done(Ok(report)),
            Err(err) => Prepared::Done(Err(err)),
        }
    }

    /// Completes a job under the lock: journals the outcome and prunes the tree. A free-space
    /// outcome just carries its result; a cleanup step may hand back the next step of the run.
    pub fn finish_job(&mut self, outcome: JobOutcome) -> Prepared {
        match outcome.0 {
            Done::Value(result) => Prepared::Done(result),
            Done::Removal(done) => {
                Prepared::Done(guard::guarded("file.delete_permanent", || file::finish_removal(self, done)))
            }
            Done::Cleanup(done) => {
                match guard::guarded("cleanup.run", || Ok(cleanup_job(cleanup::finish_step(self, done)))) {
                    Ok(Ok(job)) => Prepared::Job(job),
                    Ok(Err(report)) => Prepared::Done(Ok(report)),
                    Err(err) => Prepared::Done(Err(err)),
                }
            }
        }
    }

    /// How long a volume query may take before `platform.free_space` gives up (default 3 s).
    pub fn set_volume_query_timeout(&mut self, timeout: std::time::Duration) {
        self.volume_query_timeout = timeout;
    }

    /// True while a detached permanent deletion is running.
    pub fn removal_in_progress(&self) -> bool {
        self.removal_busy.load(Ordering::SeqCst)
    }
}

/// `Ok(job)` for the next action of a cleanup run, `Err(report)` when the run is over.
fn cleanup_job(next: Next) -> Result<DetachedJob, Value> {
    match next {
        Next::Act(step) => Ok(DetachedJob(Kind::Cleanup(step))),
        Next::Done(report) => Err(report),
    }
}

/// Admits the command (enabled, confirmation, busy gate) and runs its first phase under the guard.
/// `Ok(Ok(job))` is a job to run, `Ok(Err(report))` a result that is already complete (a dry run).
fn begin(
    session: &Session,
    id: &str,
    params: &Value,
    first_phase: impl FnOnce(&Session, &Value) -> Result<Result<DetachedJob, Value>, CommandError>,
) -> Result<Result<DetachedJob, Value>, CommandError> {
    let spec = registry().iter().find(|s| s.id == id).ok_or_else(|| CommandError::UnknownCommand(id.to_owned()))?;
    guard::guarded(id, || {
        let params = admit(session, spec, params)?;
        first_phase(session, &params)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirustat_platform::{FakePlatform, PlatformServices, SpaceInfo};
    use serde_json::json;
    use std::path::Path;
    use std::sync::{Condvar, Mutex};
    use std::time::{Duration, Instant};

    /// `root/{keep.txt, sub/{a.txt, deep/b.txt}}` scanned, with a fake platform.
    fn fixture() -> (tempfile::TempDir, Session) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("sub").join("deep")).unwrap();
        std::fs::write(root.join("keep.txt"), b"keep").unwrap();
        std::fs::write(root.join("sub").join("a.txt"), b"aaaa").unwrap();
        std::fs::write(root.join("sub").join("deep").join("b.txt"), b"bb").unwrap();
        let mut s = Session::new();
        let handle = dirustat_scan::start(&root, dirustat_scan::ScanOptions::default()).unwrap();
        assert_eq!(s.finish_scan(handle), Ok(true));
        s.set_platform(Arc::new(FakePlatform::new()));
        (dir, s)
    }

    fn perm(path: &Path) -> Value {
        json!({"path": path.to_string_lossy(), "confirm": true, "i_understand_permanent": true})
    }

    fn job(prepared: Prepared) -> DetachedJob {
        match prepared {
            Prepared::Job(job) => job,
            Prepared::Done(done) => panic!("expected a job, got {done:?}"),
        }
    }

    fn done(prepared: Prepared) -> Result<Value, CommandError> {
        match prepared {
            Prepared::Done(done) => done,
            Prepared::Job(job) => panic!("expected a finished command, got {job:?}"),
        }
    }

    /// QA-144: while the removal runs, the session is free for other commands and refuses the ones
    /// that could invalidate the plan; finishing journals and prunes.
    #[test]
    fn permanent_removal_runs_without_holding_the_session() {
        let (dir, mut s) = fixture();
        let sub = dir.path().join("root").join("sub");
        let rev = s.revision();
        let job = job(s.execute_split("file.delete_permanent", &perm(&sub)));
        // Nothing has been touched yet; the lock-free window is open.
        assert!(sub.exists() && s.revision() == rev);
        assert!(s.removal_in_progress());
        assert_eq!(s.execute("session.state", &json!({})).unwrap()["removal_in_progress"], true);
        assert!(s.execute("tree.children", &json!({})).is_ok());
        assert!(s.execute("search.find", &json!({"query": "a"})).is_ok());
        for (id, params) in [
            ("file.delete", json!({"path": sub.to_string_lossy(), "confirm": true})),
            ("file.delete_permanent", perm(&sub)),
            ("file.move", json!({"path": sub.to_string_lossy(), "destination": "x", "confirm": true})),
            ("cleanup.run", json!({"id": "temporary-files", "confirm": true})),
            ("scan.start", json!({"path": dir.path().to_string_lossy()})),
        ] {
            let err = s.execute(id, &params).unwrap_err();
            assert_eq!(err.code(), "not_enabled", "{id}: {err}");
            assert!(err.to_string().contains("still running"), "{id}: {err}");
            let split = done(s.execute_split(id, &params)).unwrap_err();
            assert_eq!(split.code(), "not_enabled", "{id}");
        }
        assert!(sub.exists());
        // The slow part needs no session at all.
        let outcome = std::thread::spawn(move || job.run()).join().unwrap();
        assert!(!sub.exists());
        assert!(s.removal_in_progress(), "the slot is held until the outcome is completed");
        assert_eq!(s.execute("file.journal", &json!({})).unwrap()["total"], 0);
        let out = done(s.finish_job(outcome)).unwrap();
        assert_eq!((out["done"].clone(), out["tree_updated"].clone()), (json!(true), json!(true)));
        assert!(!s.removal_in_progress());
        assert!(s.revision() > rev);
        let journal = s.execute("file.journal", &json!({})).unwrap();
        assert_eq!((journal["total"].clone(), journal["entries"][0]["mode"].clone()), (json!(1), json!("permanent")));
        assert_eq!(s.tree().len(), 2, "root and keep.txt are left");
        // The slot is free again.
        let keep = dir.path().join("root").join("keep.txt");
        assert_eq!(s.execute("file.delete_permanent", &perm(&keep)).unwrap()["done"], true);
    }

    #[test]
    fn dry_runs_and_refusals_finish_under_the_lock() {
        let (dir, mut s) = fixture();
        let sub = dir.path().join("root").join("sub");
        let dry = json!({"path": sub.to_string_lossy(), "dry_run": true, "confirm": true});
        let out = done(s.execute_split("file.delete_permanent", &dry)).unwrap();
        assert_eq!(out["dry_run"], true);
        assert!(!s.removal_in_progress());
        for (params, code) in [
            (json!({"path": sub.to_string_lossy()}), "confirmation_required"),
            (json!({"path": sub.to_string_lossy(), "confirm": true}), "confirmation_required"),
            (json!({"path": "C:\\nope\\x", "confirm": true, "i_understand_permanent": true}), "invalid_params"),
            (json!({"bogus": 1, "confirm": true, "i_understand_permanent": true}), "invalid_params"),
        ] {
            assert_eq!(done(s.execute_split("file.delete_permanent", &params)).unwrap_err().code(), code);
            assert!(!s.removal_in_progress());
        }
        // Any other command is simply executed.
        assert!(done(s.execute_split("session.state", &json!({}))).is_ok());
        assert_eq!(done(s.execute_split("no.such", &json!({}))).unwrap_err().code(), "unknown_command");
        assert!(sub.exists());
    }

    /// A job that is dropped without running frees the slot (a crashed host must not wedge deletes).
    #[test]
    fn dropping_a_job_releases_the_slot() {
        let (dir, mut s) = fixture();
        let sub = dir.path().join("root").join("sub");
        let pending = job(s.execute_split("file.delete_permanent", &perm(&sub)));
        assert!(s.removal_in_progress());
        drop(pending);
        assert!(!s.removal_in_progress());
        assert!(sub.exists());
        assert_eq!(s.execute("file.delete_permanent", &perm(&sub)).unwrap()["done"], true);
    }

    /// If the tree was replaced while the removal ran, the wrong tree is not pruned: the outcome
    /// is journaled and a rescan is recommended.
    #[test]
    fn a_tree_replaced_during_the_removal_is_not_pruned() {
        let (dir, mut s) = fixture();
        let sub = dir.path().join("root").join("sub");
        let job = job(s.execute_split("file.delete_permanent", &perm(&sub)));
        let outcome = job.run();
        let mut other = dirustat_model::Tree::new("other");
        other.add_child(other.root(), "f", dirustat_model::NodeKind::File, 1).unwrap();
        other.aggregate();
        s.replace_tree(other);
        let nodes = s.tree().len();
        let out = done(s.finish_job(outcome)).unwrap();
        assert_eq!((out["done"].clone(), out["tree_updated"].clone()), (json!(true), json!(false)));
        assert_eq!(out["rescan_recommended"], true);
        assert_eq!(s.tree().len(), nodes, "the replacement tree is untouched");
        let state = s.execute("session.state", &json!({})).unwrap();
        assert_eq!(state["rescan_recommended"], true);
        assert_eq!(state["removal_in_progress"], false);
        assert_eq!(s.execute("file.journal", &json!({})).unwrap()["total"], 1);
    }

    /// A removal that fails is journaled as failed and still releases the slot.
    #[test]
    fn a_failed_removal_is_reported_and_releases_the_slot() {
        let (dir, mut s) = fixture();
        let sub = dir.path().join("root").join("sub");
        let job = job(s.execute_split("file.delete_permanent", &perm(&sub)));
        std::fs::remove_dir_all(&sub).unwrap(); // somebody else got there first
        let err = done(s.finish_job(job.run())).unwrap_err();
        assert_eq!(err.code(), "internal");
        assert!(!s.removal_in_progress());
        let journal = s.execute("file.journal", &json!({})).unwrap();
        assert_eq!(journal["entries"][0]["ok"], false);
        assert_eq!(s.tree().len(), 2, "the node that is gone is pruned");
        assert_eq!(s.execute("session.state", &json!({})).unwrap()["rescan_recommended"], true);
    }

    /// A platform whose volume query blocks until it is released.
    #[derive(Debug, Default)]
    struct StuckPlatform {
        gate: Arc<(Mutex<bool>, Condvar)>,
    }

    impl StuckPlatform {
        fn release(&self) {
            *self.gate.0.lock().unwrap() = true;
            self.gate.1.notify_all();
        }
    }

    impl PlatformServices for StuckPlatform {
        fn drives(&self) -> dirustat_platform::Result<Vec<dirustat_platform::DriveInfo>> {
            Ok(Vec::new())
        }
        fn free_space(&self, _path: &Path) -> dirustat_platform::Result<SpaceInfo> {
            let (lock, cv) = &*self.gate;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = cv.wait(released).unwrap();
            }
            Ok(SpaceInfo { total: 10, free: 4 })
        }
        fn move_to_recycle_bin(&self, _path: &Path) -> dirustat_platform::Result<()> {
            Ok(())
        }
        fn reveal_in_file_manager(&self, _path: &Path) -> dirustat_platform::Result<()> {
            Ok(())
        }
        fn show_properties(&self, _path: &Path) -> dirustat_platform::Result<()> {
            Ok(())
        }
    }

    /// QA-143: the OS call is made without the session, and a stuck volume costs one helper
    /// thread, not one per call: later calls are refused at once until the OS answers.
    #[test]
    fn a_stuck_volume_holds_neither_the_session_nor_more_than_one_thread() {
        let (dir, mut s) = fixture();
        let stuck = Arc::new(StuckPlatform::default());
        s.set_platform(stuck.clone());
        s.set_volume_query_timeout(Duration::from_millis(250));
        let params = json!({"path": dir.path().to_string_lossy()});
        let first = job(s.execute_split("platform.free_space", &params));
        // The session is free while the (stuck) OS call is pending.
        let waiter = std::thread::spawn(move || first.run());
        std::thread::sleep(Duration::from_millis(60));
        let t = Instant::now();
        assert!(s.execute("session.state", &json!({})).is_ok());
        assert!(t.elapsed() < Duration::from_millis(100), "{:?}", t.elapsed());
        let timed_out = done(s.finish_job(waiter.join().unwrap())).unwrap_err();
        assert!(timed_out.to_string().contains("in time"), "{timed_out}");
        // The helper is still stuck: a second call does not start another thread and does not wait.
        for _ in 0..5 {
            let again = job(s.execute_split("platform.free_space", &params));
            let t = Instant::now();
            let err = done(s.finish_job(again.run())).unwrap_err();
            assert!(t.elapsed() < Duration::from_millis(100), "{:?}", t.elapsed());
            assert!(err.to_string().contains("has not returned yet"), "{err}");
        }
        // The in-lock path behaves the same.
        let err = s.execute("platform.free_space", &params).unwrap_err();
        assert!(err.to_string().contains("has not returned yet"), "{err}");
        // Once the OS answers, the next call works.
        stuck.release();
        let deadline = Instant::now() + Duration::from_secs(5);
        let out = loop {
            match s.execute("platform.free_space", &params) {
                Ok(out) => break out,
                Err(err) => {
                    assert!(Instant::now() < deadline, "the helper never finished: {err}");
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        };
        assert_eq!((out["total"].clone(), out["free"].clone()), (json!(10), json!(4)));
    }

    #[test]
    fn free_space_validates_before_any_thread_starts() {
        let (_dir, mut s) = fixture();
        for p in [json!({}), json!({"path": "relative"}), json!({"path": "\\\\server\\share"}), json!({"x": 1})] {
            assert_eq!(done(s.execute_split("platform.free_space", &p)).unwrap_err().code(), "invalid_params");
        }
    }

    /// Saves a permanent cleanup of every `*.txt` file and returns its confirmed run params.
    fn txt_cleanup(s: &mut Session) -> Value {
        let def = json!({"id": "txt", "name": "Txt", "action": "permanent", "selector": {"names": ["*.txt"], "kind": "file"}});
        s.execute("cleanup.save", &json!({ "definition": def })).unwrap();
        json!({"id": "txt", "confirm": true, "i_understand_permanent": true})
    }

    /// G-062: a cleanup run is a chain of jobs, one per action. Between them the session is free
    /// for other commands and refuses the ones that could invalidate the plan; the tree is pruned
    /// once, at the end.
    #[test]
    fn cleanup_runs_act_without_holding_the_session() {
        let (dir, mut s) = fixture();
        let root = dir.path().join("root");
        let params = txt_cleanup(&mut s);
        let rev = s.revision();
        let mut prepared = s.execute_split("cleanup.run", &params);
        let mut actions = 0;
        let out = loop {
            let job = match prepared {
                Prepared::Done(result) => break result.unwrap(),
                Prepared::Job(job) => job,
            };
            actions += 1;
            assert!(s.removal_in_progress());
            assert_eq!(s.revision(), rev, "nothing is pruned while the run is in flight");
            assert!(s.execute("tree.children", &json!({})).is_ok());
            let err = s.execute("scan.start", &json!({"path": dir.path().to_string_lossy()})).unwrap_err();
            assert_eq!(err.code(), "not_enabled");
            assert_eq!(done(s.execute_split("cleanup.run", &params)).unwrap_err().code(), "not_enabled");
            let outcome = std::thread::spawn(move || job.run()).join().unwrap();
            assert_eq!(s.execute("file.journal", &json!({})).unwrap()["total"], actions - 1, "journaled on finish");
            prepared = s.finish_job(outcome);
        };
        assert_eq!(actions, 3);
        assert_eq!((out["deleted"].clone(), out["tree_updated"].clone()), (json!(3), json!(true)));
        assert!(!s.removal_in_progress());
        assert_eq!(s.revision(), rev + 1);
        assert_eq!(s.execute("file.journal", &json!({})).unwrap()["total"], 3);
        assert!(!root.join("keep.txt").exists() && !root.join("sub").join("a.txt").exists());
        assert!(root.join("sub").join("deep").exists());
    }

    /// If the tree is replaced between two actions, the run stops, nothing of the new tree is
    /// pruned and a rescan is recommended.
    #[test]
    fn a_tree_replaced_during_a_cleanup_stops_the_run() {
        let (dir, mut s) = fixture();
        let params = txt_cleanup(&mut s);
        let first = job(s.execute_split("cleanup.run", &params));
        let outcome = first.run();
        let mut other = dirustat_model::Tree::new("other");
        other.add_child(other.root(), "f.txt", dirustat_model::NodeKind::File, 1).unwrap();
        other.aggregate();
        s.replace_tree(other);
        let out = done(s.finish_job(outcome)).unwrap();
        assert_eq!((out["deleted"].clone(), out["not_attempted"].clone()), (json!(1), json!(2)));
        assert!(out["stopped"].as_str().unwrap().contains("changed"), "{out}");
        assert_eq!((out["tree_updated"].clone(), out["rescan_recommended"].clone()), (json!(false), json!(true)));
        assert_eq!(s.tree().len(), 2, "the replacement tree is untouched");
        assert_eq!(s.execute("session.state", &json!({})).unwrap()["rescan_recommended"], true);
        assert!(!s.removal_in_progress());
        let root = dir.path().join("root");
        let left = [root.join("keep.txt"), root.join("sub").join("a.txt"), root.join("sub").join("deep").join("b.txt")];
        assert_eq!(left.iter().filter(|p| p.exists()).count(), 2, "only one item was removed");
    }

    #[test]
    fn dropping_a_cleanup_job_releases_the_slot_and_dry_runs_never_take_it() {
        let (dir, mut s) = fixture();
        let params = txt_cleanup(&mut s);
        let mut dry = params.clone();
        dry["dry_run"] = json!(true);
        assert_eq!(done(s.execute_split("cleanup.run", &dry)).unwrap()["results"].as_array().unwrap().len(), 3);
        assert!(!s.removal_in_progress());
        drop(job(s.execute_split("cleanup.run", &params)));
        assert!(!s.removal_in_progress());
        assert!(dir.path().join("root").join("keep.txt").exists());
        assert_eq!(s.execute("cleanup.run", &params).unwrap()["deleted"], 3);
    }

    /// A platform that records whether the shared session was free while it recycled an item.
    #[derive(Debug, Default)]
    struct LockProbe {
        session: std::sync::OnceLock<std::sync::Weak<Mutex<Session>>>,
        free_during_action: Mutex<Vec<bool>>,
    }

    impl PlatformServices for LockProbe {
        fn drives(&self) -> dirustat_platform::Result<Vec<dirustat_platform::DriveInfo>> {
            Ok(Vec::new())
        }
        fn free_space(&self, _path: &Path) -> dirustat_platform::Result<SpaceInfo> {
            Ok(SpaceInfo { total: 1, free: 1 })
        }
        fn move_to_recycle_bin(&self, path: &Path) -> dirustat_platform::Result<()> {
            let free = self.session.get().and_then(std::sync::Weak::upgrade).is_some_and(|m| m.try_lock().is_ok());
            self.free_during_action.lock().unwrap().push(free);
            std::fs::remove_file(path)
                .map_err(|e| dirustat_platform::PlatformError::Failed { op: "recycle", message: e.to_string() })
        }
        fn reveal_in_file_manager(&self, _path: &Path) -> dirustat_platform::Result<()> {
            Ok(())
        }
        fn show_properties(&self, _path: &Path) -> dirustat_platform::Result<()> {
            Ok(())
        }
    }

    /// `execute_unlocked` runs a recycle cleanup to the end with the session released for every
    /// action, and plain commands simply run.
    #[test]
    fn execute_unlocked_releases_the_lock_for_every_action() {
        let (_dir, mut s) = fixture();
        let probe = Arc::new(LockProbe::default());
        s.set_platform(probe.clone());
        let def =
            json!({"id": "bin", "name": "Bin", "action": "recycle", "selector": {"names": ["*.txt"], "kind": "file"}});
        s.execute("cleanup.save", &json!({ "definition": def })).unwrap();
        let shared = Arc::new(Mutex::new(s));
        probe.session.set(Arc::downgrade(&shared)).unwrap();
        let lock = || shared.lock().unwrap();
        let out = execute_unlocked(lock, "cleanup.run", &json!({"id": "bin", "confirm": true})).unwrap();
        assert_eq!(out["deleted"], 3);
        assert_eq!(*probe.free_during_action.lock().unwrap(), [true, true, true]);
        assert!(execute_unlocked(lock, "session.state", &json!({})).is_ok());
        assert_eq!(execute_unlocked(lock, "no.such", &json!({})).unwrap_err().code(), "unknown_command");
        assert!(!lock().removal_in_progress());
    }
}
