//! `UiBridge` for the desktop app: control-server threads hand requests to the UI thread through
//! a channel and wait a bounded time for the answer. The UI thread drains the channel once per
//! frame ([`UiEndpoint::pump`]); the egui context is woken so an idle window still answers.

use dirustat_control::{ControlError, ScreenshotRequest, UiBridge};
use dirustat_ui::{DirustatApp, Region, SaveOptions, ScreenshotStatus, UiState};
use eframe::egui;
use serde_json::{Value, json};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How long a control thread waits for the UI thread to pick a request up. Frames normally run
/// every few milliseconds; no pickup within this time means the window is not rendering (for
/// example it is minimized), so the call fails fast with an actionable error.
pub const ACK_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a control thread waits for the answer of a request the UI has picked up
/// (`ui.set_state`, `app.quit`, and the margin after a screenshot).
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a screenshot may stay in flight after the UI accepted it before it is answered with a
/// timeout error. The control call can take this plus [`REPLY_TIMEOUT`] at most (40 s).
pub const SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(30);
/// Requests handled per frame, so a flood cannot starve drawing.
const MAX_PER_FRAME: usize = 32;
/// Repaint interval while a screenshot is in flight.
const POLL_REPAINT: Duration = Duration::from_millis(30);

type Reply = Result<Value, ControlError>;
/// Latest serialized UI state, published by the UI thread whenever it changes.
type Snapshot = Arc<Mutex<Option<Value>>>;
type Wake = Arc<Mutex<Option<egui::Context>>>;

#[derive(Debug)]
enum Request {
    State,
    SetState(Value),
    Screenshot(ScreenshotRequest),
    Quit,
}

/// What the UI thread sends back: an interim pickup notice for slow requests, then the answer.
#[derive(Debug)]
enum Msg {
    Accepted,
    Done(Reply),
}

#[derive(Debug)]
struct Envelope {
    request: Request,
    reply: Sender<Msg>,
}

/// Control-thread side.
#[derive(Debug)]
pub struct ChannelBridge {
    tx: Mutex<Sender<Envelope>>,
    wake: Wake,
    snapshot: Snapshot,
    ack_timeout: Duration,
    reply_timeout: Duration,
    screenshot_timeout: Duration,
}

/// UI-thread side: call [`UiEndpoint::pump`] once per frame.
#[derive(Debug)]
pub struct UiEndpoint {
    rx: Receiver<Envelope>,
    wake: Wake,
    snapshot: Snapshot,
    published: Option<UiState>,
    pending: Vec<PendingShot>,
    screenshot_timeout: Duration,
}

#[derive(Debug)]
struct PendingShot {
    ticket: u64,
    deadline: Instant,
    reply: Sender<Msg>,
}

/// Creates a connected bridge and endpoint with the default timeouts.
pub fn channel() -> (ChannelBridge, UiEndpoint) {
    channel_with_timeouts(REPLY_TIMEOUT, SCREENSHOT_TIMEOUT)
}

/// Like [`channel`] with explicit timeouts (tests use short ones).
pub fn channel_with_timeouts(reply_timeout: Duration, screenshot_timeout: Duration) -> (ChannelBridge, UiEndpoint) {
    let (tx, rx) = mpsc::channel();
    let wake: Wake = Arc::new(Mutex::new(None));
    let snapshot: Snapshot = Arc::new(Mutex::new(None));
    let bridge = ChannelBridge {
        tx: Mutex::new(tx),
        wake: Arc::clone(&wake),
        snapshot: Arc::clone(&snapshot),
        ack_timeout: ACK_TIMEOUT.min(reply_timeout),
        reply_timeout,
        screenshot_timeout,
    };
    let endpoint = UiEndpoint { rx, wake, snapshot, published: None, pending: Vec::new(), screenshot_timeout };
    (bridge, endpoint)
}

/// Why a request got no answer in time.
enum Wait {
    /// The UI thread never picked the request up: the window is not rendering.
    NotPickedUp,
    Failed(ControlError),
}

impl ChannelBridge {
    /// Sends `request` and waits: up to `ack_timeout` for pickup, then up to `done_timeout` for the
    /// answer when the UI announced a slow request.
    fn call(&self, request: Request, done_timeout: Duration) -> Result<Reply, Wait> {
        let (reply_tx, reply_rx) = mpsc::channel();
        {
            let tx = self.tx.lock().unwrap_or_else(PoisonError::into_inner);
            tx.send(Envelope { request, reply: reply_tx })
                .map_err(|_| Wait::Failed(ControlError::internal("the window is closed")))?;
        }
        if let Some(ctx) = self.wake.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
            ctx.request_repaint();
        }
        let closed = || Wait::Failed(ControlError::internal("the window closed before answering"));
        match reply_rx.recv_timeout(self.ack_timeout) {
            Ok(Msg::Done(reply)) => Ok(reply),
            Ok(Msg::Accepted) => match reply_rx.recv_timeout(done_timeout) {
                Ok(Msg::Done(reply)) => Ok(reply),
                Ok(Msg::Accepted) | Err(RecvTimeoutError::Timeout) => Err(Wait::Failed(ControlError::internal(
                    format!("timeout: the UI did not finish within {}s", done_timeout.as_secs()),
                ))),
                Err(RecvTimeoutError::Disconnected) => Err(closed()),
            },
            Err(RecvTimeoutError::Timeout) => Err(Wait::NotPickedUp),
            Err(RecvTimeoutError::Disconnected) => Err(closed()),
        }
    }

    fn not_rendering(&self) -> ControlError {
        ControlError::internal(format!(
            "timeout: the window did not respond within {}s; it is probably minimized or hidden, restore it and retry",
            self.ack_timeout.as_secs()
        ))
    }

    /// Like [`Self::call`] with a plain error for an unpicked request.
    fn call_or_error(&self, request: Request, done_timeout: Duration) -> Reply {
        match self.call(request, done_timeout) {
            Ok(reply) => reply,
            Err(Wait::NotPickedUp) => Err(self.not_rendering()),
            Err(Wait::Failed(e)) => Err(e),
        }
    }
}

impl UiBridge for ChannelBridge {
    /// Served from the UI frame; when the window does not render (minimized) the last published
    /// state is returned instead, marked with `"stale": true`.
    fn ui_state(&self) -> Reply {
        match self.call(Request::State, self.reply_timeout) {
            Ok(reply) => reply,
            Err(Wait::NotPickedUp) => {
                let last = self.snapshot.lock().unwrap_or_else(PoisonError::into_inner).clone();
                match last {
                    Some(Value::Object(mut map)) => {
                        map.insert("stale".to_owned(), Value::Bool(true));
                        Ok(Value::Object(map))
                    }
                    _ => Err(self.not_rendering()),
                }
            }
            Err(Wait::Failed(e)) => Err(e),
        }
    }

    fn ui_set_state(&self, patch: &Value) -> Reply {
        self.call_or_error(Request::SetState(patch.clone()), self.reply_timeout)
    }

    fn ui_screenshot(&self, request: &ScreenshotRequest) -> Reply {
        // After pickup the UI answers once the capture is done (its own deadline is
        // `screenshot_timeout`); allow a margin.
        let wait = self.screenshot_timeout.saturating_add(self.reply_timeout);
        self.call_or_error(Request::Screenshot(request.clone()), wait)
    }

    fn app_quit(&self) -> Reply {
        self.call_or_error(Request::Quit, self.reply_timeout)
    }
}

fn region_f32(r: dirustat_control::Region) -> Region {
    // Values are validated finite and non-negative by the dispatcher; f32 precision is plenty.
    Region { x: r.x as f32, y: r.y as f32, w: r.w as f32, h: r.h as f32 }
}

impl UiEndpoint {
    /// Lets bridge threads wake the UI loop. Call once the egui context exists.
    pub fn attach(&self, ctx: &egui::Context) {
        *self.wake.lock().unwrap_or_else(PoisonError::into_inner) = Some(ctx.clone());
    }

    /// Tickets of screenshots still waiting for the backend.
    #[cfg(test)]
    pub fn pending_tickets(&self) -> Vec<u64> {
        self.pending.iter().map(|p| p.ticket).collect()
    }

    /// Handles queued requests and finished screenshots. Never blocks.
    pub fn pump(&mut self, app: &mut DirustatApp, ctx: &egui::Context) {
        for _ in 0..MAX_PER_FRAME {
            let Ok(Envelope { request, reply }) = self.rx.try_recv() else { break };
            self.handle(request, reply, app, ctx);
        }
        self.poll_screenshots(app);
        self.publish(app);
        if !self.pending.is_empty() {
            ctx.request_repaint_after(POLL_REPAINT);
        }
    }

    /// Publishes the serialized UI state for requests that arrive while no frame runs.
    fn publish(&mut self, app: &DirustatApp) {
        if self.published.as_ref() == Some(app.state()) {
            return;
        }
        self.published = Some(app.state().clone());
        *self.snapshot.lock().unwrap_or_else(PoisonError::into_inner) = Some(app.ui_state_json());
    }

    fn handle(&mut self, request: Request, reply: Sender<Msg>, app: &mut DirustatApp, ctx: &egui::Context) {
        let answer = match request {
            Request::State => Ok(app.ui_state_json()),
            Request::SetState(patch) => {
                app.apply_ui_patch(&patch).map(|()| app.ui_state_json()).map_err(ControlError::invalid_params)
            }
            Request::Quit => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                Ok(json!({}))
            }
            Request::Screenshot(shot) => {
                let path = shot.path.to_string_lossy().into_owned();
                let options = SaveOptions::controlled(shot.overwrite);
                match app.request_screenshot_with(&path, shot.region.map(region_f32), options) {
                    Ok(ticket) => {
                        let deadline = Instant::now() + self.screenshot_timeout;
                        let _ = reply.send(Msg::Accepted);
                        self.pending.push(PendingShot { ticket, deadline, reply });
                        ctx.request_repaint();
                        return;
                    }
                    Err(message) => Err(ControlError::invalid_params(message)),
                }
            }
        };
        let _ = reply.send(Msg::Done(answer));
    }

    fn poll_screenshots(&mut self, app: &DirustatApp) {
        let now = Instant::now();
        self.pending.retain(|p| {
            let answer = match app.screenshot_status(p.ticket) {
                Some(ScreenshotStatus::Done { path, width, height }) => {
                    Ok(json!({ "path": path, "width": width, "height": height }))
                }
                Some(ScreenshotStatus::Failed { message }) => Err(ControlError::internal(message)),
                Some(ScreenshotStatus::Pending) if now < p.deadline => return true,
                Some(ScreenshotStatus::Pending) => Err(ControlError::internal("timeout: screenshot was not captured")),
                None => Err(ControlError::internal("screenshot request expired")),
            };
            let _ = p.reply.send(Msg::Done(answer));
            false
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirustat_control::{Backend, ControlClient, ControlServer};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    fn input() -> egui::RawInput {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        egui::RawInput { screen_rect: Some(screen), ..egui::RawInput::default() }
    }

    /// One headless frame: draw the app, pump the endpoint; feeds a fake capture for pending tickets.
    fn frame(app: &mut DirustatApp, endpoint: &mut UiEndpoint, ctx: &egui::Context) {
        let mut raw = input();
        for ticket in endpoint.pending_tickets() {
            raw.events.push(egui::Event::Screenshot {
                viewport_id: egui::ViewportId::ROOT,
                user_data: egui::UserData::new(ticket),
                image: Arc::new(egui::ColorImage::filled([32, 24], egui::Color32::WHITE)),
            });
        }
        let out = ctx.run_ui(raw, |ui| app.show(ui));
        endpoint.pump(app, ctx);
        out.drop_without_applying_deltas();
    }

    /// Drives frames until the client thread sets `done`.
    fn drive(app: &mut DirustatApp, endpoint: &mut UiEndpoint, done: &AtomicBool) {
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !done.load(Ordering::SeqCst) && Instant::now() < deadline {
            frame(app, endpoint, &ctx);
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn timeout_when_the_ui_never_answers_and_error_when_gone() {
        let (bridge, endpoint) = channel_with_timeouts(Duration::from_millis(50), Duration::from_millis(50));
        let started = Instant::now();
        let err = bridge.ui_state().unwrap_err();
        assert_eq!(err.code(), "internal");
        assert!(err.message().starts_with("timeout"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));
        drop(endpoint);
        assert_eq!(bridge.ui_state().unwrap_err().code(), "internal");
        assert_eq!(bridge.app_quit().unwrap_err().code(), "internal");
    }

    #[test]
    fn minimized_window_gets_a_stale_state_and_a_fast_actionable_screenshot_error() {
        let (bridge, mut endpoint) = channel_with_timeouts(Duration::from_millis(100), Duration::from_secs(30));
        let mut app = DirustatApp::new();
        // Before any frame there is no snapshot: plain timeout error.
        assert!(bridge.ui_state().unwrap_err().message().contains("minimized"));
        let ctx = egui::Context::default();
        frame(&mut app, &mut endpoint, &ctx);
        // No more frames (minimized): the last published state answers, marked stale.
        let state = bridge.ui_state().unwrap();
        assert_eq!(state["stale"], json!(true));
        assert_eq!(state["panels"]["tree"], json!(true));
        let dir = tempfile::tempdir().unwrap();
        let request = ScreenshotRequest { path: dir.path().join("x.png"), region: None, overwrite: false };
        let started = Instant::now();
        let err = bridge.ui_screenshot(&request).unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(err.message().contains("restore it"), "{err}");
    }

    #[test]
    fn state_patch_and_quit_through_the_ui_loop() {
        let (bridge, mut endpoint) = channel();
        let mut app = DirustatApp::new();
        let done = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&done);
        let client = thread::spawn(move || {
            let state = bridge.ui_state();
            let patched = bridge.ui_set_state(&json!({"panels": {"tree": false}}));
            let bad = bridge.ui_set_state(&json!({"bogus": 1}));
            let quit = bridge.app_quit();
            flag.store(true, Ordering::SeqCst);
            (state, patched, bad, quit)
        });
        drive(&mut app, &mut endpoint, &done);
        let (state, patched, bad, quit) = client.join().unwrap();
        assert_eq!(state.unwrap()["panels"]["tree"], json!(true));
        assert_eq!(patched.unwrap()["panels"]["tree"], json!(false));
        assert_eq!(bad.unwrap_err().code(), "invalid_params");
        assert!(quit.is_ok());
        assert!(!app.state().panels.tree);
    }

    #[test]
    fn unanswered_screenshot_times_out_on_the_ui_side() {
        let (bridge, mut endpoint) = channel_with_timeouts(Duration::from_secs(5), Duration::from_millis(50));
        let mut app = DirustatApp::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("never.png");
        let request = ScreenshotRequest { path, region: None, overwrite: false };
        let client = thread::spawn(move || bridge.ui_screenshot(&request));
        let ctx = egui::Context::default();
        // Frames without any capture event: the pending screenshot must expire.
        let deadline = Instant::now() + Duration::from_secs(20);
        while !client.is_finished() && Instant::now() < deadline {
            let out = ctx.run_ui(input(), |ui| app.show(ui));
            endpoint.pump(&mut app, &ctx);
            out.drop_without_applying_deltas();
            thread::sleep(Duration::from_millis(5));
        }
        let err = client.join().unwrap().unwrap_err();
        assert!(err.message().starts_with("timeout"), "{err}");
    }

    #[test]
    fn end_to_end_over_tcp_with_a_shared_session() {
        let dir = tempfile::tempdir().unwrap();
        let (bridge, mut endpoint) = channel();
        let mut app = DirustatApp::new();
        let server = ControlServer::start(0, dir.path(), app.shared_session(), Arc::new(bridge)).unwrap();
        let addr = format!("127.0.0.1:{}", server.port());
        let key = server.key().to_owned();
        let png = dir.path().join("shot.png");
        let png_text = png.to_string_lossy().into_owned();
        let done = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&done);
        let client = thread::spawn(move || {
            let c = ControlClient::connect(&addr, &key).map_err(|e| e.to_string())?;
            let mode = c.call("engine.execute", &json!({"command": "view.size_mode", "params": {"mode": "logical"}}));
            let ui = c.call("ui.state", &json!({}));
            let shot = c.call("ui.screenshot", &json!({"path": png_text}));
            let bad = c.call("ui.screenshot", &json!({"path": "no-extension"}));
            let again = c.call("ui.screenshot", &json!({"path": png_text}));
            let over = c.call("ui.screenshot", &json!({"path": png_text, "overwrite": true}));
            let quit = c.call("app.quit", &json!({}));
            flag.store(true, Ordering::SeqCst);
            Ok::<_, String>((mode, ui, shot, bad, again, over, quit))
        });
        drive(&mut app, &mut endpoint, &done);
        let (mode, ui, shot, bad, again, over, quit) = client.join().unwrap().unwrap();
        assert!(mode.is_ok(), "{mode:?}");
        assert_eq!(ui.unwrap()["panels"]["map"], json!(true));
        assert_eq!(shot.unwrap()["width"], 32);
        assert!(png.is_file());
        assert!(bad.is_err());
        // The second capture of the same path is refused unless `overwrite` is passed.
        assert_eq!(again.unwrap_err().code(), "invalid_params");
        assert!(over.is_ok());
        assert!(quit.is_ok());
        // The command ran on the very session the UI draws.
        assert_eq!(app.session().map_size_mode(), dirustat_model::SizeMode::Logical);
        server.shutdown();
    }
}
