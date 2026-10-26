//! Single-instance IPC: serving requests from the CLI, `snapoleon://` URLs and second launches,
//! and forwarding this process's own request to the instance that already runs. On macOS the
//! system delivers links, documents and second launches to the running app itself (`OsRequest`);
//! they take the same paths.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use crossbeam_channel::Sender;
use snap_core::{
    Result,
    hotkey::Action,
    ipc::{AppWindow, Request, Response},
    t,
};
use snap_engine::{CaptureRequest, Notice, NoticeKind};
use snap_platform::ipc::Server;
use snap_shell::{OsRequest, Proxy, Shell};

use crate::{
    actions::mode_work,
    app::App,
    commands::{Command, Work},
    events::AppEvent,
    notices::error_text,
    views::overlay::OverlayMode,
};

/// How long a client waits for the app to answer. Interactive requests answer at once;
/// non-interactive captures answer when the pipeline has finished.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(300);

/// Start serving. Fails with `AlreadyRunning` when another instance owns the endpoint.
pub fn serve(proxy: Proxy<AppEvent>) -> Result<Server> {
    snap_platform::ipc::serve(move |request| {
        let (reply, answer) = crossbeam_channel::bounded(1);
        proxy.send(AppEvent::Ipc(request, reply));
        answer.recv_timeout(ANSWER_TIMEOUT).unwrap_or_else(|_| Response::err(t("app.ipc.no_answer")))
    })
}

/// Hand this launch over to the running instance: its request, or the Library for a plain launch.
/// Prints what went wrong and returns false when the request failed.
pub fn forward(request: Option<Request>, background: bool) -> bool {
    let request = match (request, background) {
        (Some(request), _) => request,
        (None, false) => Request::ShowWindow { window: AppWindow::Library },
        (None, true) => return true,
    };
    allow_foreground();
    match snap_platform::ipc::send(&request) {
        Ok(response) if response.ok => {
            // Text results (`ocr`, `process`) are what a script is after.
            for line in [&response.path, &response.url, &response.text].into_iter().flatten() {
                println!("{line}");
            }
            true
        }
        Ok(response) => {
            eprintln!("{}", response.error.unwrap_or_default());
            false
        }
        Err(e) => {
            eprintln!("{}", error_text(&e));
            false
        }
    }
}

/// Windows only lets the foreground process hand focus on; do it before asking the running
/// instance to show a window, or the window would open behind the one the user is looking at.
#[cfg(windows)]
fn allow_foreground() {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn AllowSetForegroundWindow(process_id: u32) -> i32;
    }
    const ASFW_ANY: u32 = u32::MAX;
    // SAFETY: plain Win32 call with a constant argument.
    unsafe { AllowSetForegroundWindow(ASFW_ANY) };
}

#[cfg(not(windows))]
fn allow_foreground() {}

/// What a `snapoleon://` link may start: always an action where the user still makes the choice (overlay, picker).
/// Captures that would fire at once, and recording and step sessions, are turned into their interactive form or
/// refused, so a web page can neither photograph the screen nor start recording behind the user's back.
fn link_action(action: Action) -> Option<Action> {
    match action {
        Action::CaptureMonitor | Action::CaptureAllMonitors | Action::CaptureLastRegion => Some(Action::CaptureRegion),
        Action::RecordScreen => Some(Action::RecordRegion),
        Action::StepCaptureToggle => None,
        action => Some(action),
    }
}

impl App {
    /// A request from macOS: the app menu, the Dock, Finder and LaunchServices.
    pub(crate) fn handle_os_request(&mut self, sh: &mut Shell<AppEvent>, request: OsRequest) {
        match request {
            // What a second launch does elsewhere (`forward` with no request).
            OsRequest::Reopen => {
                self.cmds.push(if self.settings.general.first_run_done { Command::OpenLibrary } else { Command::OpenOnboarding })
            }
            OsRequest::OpenUrl(url) => match snap_core::ipc::parse_url(&url) {
                Some(request) => self.handle_os_ipc(sh, request),
                None => self.os_request_failed(sh, t("app.ipc.link_refused").into_owned()),
            },
            OsRequest::OpenFile(path) => self.handle_os_ipc(sh, Request::Edit { path: path.to_string_lossy().into_owned() }),
            OsRequest::About => self.cmds.push(Command::OpenAbout),
            OsRequest::Settings => self.cmds.push(Command::OpenSettings { page: None }),
            OsRequest::Quit => self.cmds.push(Command::Quit),
        }
    }

    /// [`App::handle_request`] for a request nobody waits on: a refusal (a link that may not start its action, a
    /// file that is gone) becomes a notice instead.
    fn handle_os_ipc(&mut self, sh: &mut Shell<AppEvent>, request: Request) {
        let (reply, answer) = crossbeam_channel::bounded(1);
        self.handle_request(sh, request, Some(reply));
        if let Ok(Response { ok: false, error, .. }) = answer.try_recv() {
            self.os_request_failed(sh, error.unwrap_or_default());
        }
    }

    fn os_request_failed(&mut self, sh: &mut Shell<AppEvent>, error: String) {
        self.notice(sh, Notice { kind: NoticeKind::Error, title: error, ..Notice::default() });
    }

    /// Handle a request from the CLI, a URL or a second instance. `reply` is `None` for this
    /// process's own launch request.
    pub(crate) fn handle_request(&mut self, sh: &mut Shell<AppEvent>, request: Request, reply: Option<Sender<Response>>) {
        let answer = |reply: Option<Sender<Response>>, response: Response| {
            if let Some(reply) = reply {
                let _ = reply.send(response);
            }
        };
        match request {
            Request::Ping => answer(reply, Response::ok()),
            Request::Action { action, pipeline, region, window_title, monitor } => {
                match self.direct_command(action, pipeline.clone(), region, window_title, monitor, reply.clone()) {
                    // A non-interactive capture answers when it is done.
                    Some(command @ Command::CaptureAndRun { .. }) => self.cmds.push(command),
                    Some(command) => {
                        self.cmds.push(command);
                        answer(reply, Response::ok());
                    }
                    None => {
                        self.cmds.push(Command::StartCapture { action, pipeline });
                        answer(reply, Response::ok());
                    }
                }
            }
            Request::Link { action } => match link_action(action) {
                Some(action) => {
                    self.start_action(sh, action, None);
                    answer(reply, Response::ok());
                }
                None => answer(reply, Response::err(t("app.ipc.link_refused"))),
            },
            Request::Edit { path } => answer(reply, self.queue_file(path, Command::EditFile)),
            Request::Pin { path } => answer(reply, self.queue_file(path, Command::PinFile)),
            Request::Process { path, pipeline } => {
                if self.settings.pipeline(&pipeline).is_none() {
                    answer(reply, Response::err(snap_core::tf("app.error.no_pipeline", &[("id", &pipeline)])));
                } else {
                    // The command answers when the job is done.
                    self.cmds.push(Command::ProcessFile { path: PathBuf::from(path), pipeline, reply });
                }
            }
            Request::ShowWindow { window } => {
                self.cmds.push(match window {
                    AppWindow::Library => Command::OpenLibrary,
                    AppWindow::Settings => Command::OpenSettings { page: None },
                    AppWindow::About => Command::OpenAbout,
                });
                answer(reply, Response::ok());
            }
            Request::ReloadSettings => match self.engine.reload_settings() {
                Ok(()) => {
                    self.reload_settings(sh);
                    answer(reply, Response::ok());
                }
                Err(e) => answer(reply, Response::err(error_text(&e))),
            },
            Request::Quit => {
                answer(reply, Response::ok());
                self.cmds.push(Command::Quit);
            }
            // A headless `snapoleon capture` or `snapoleon mcp` that may not read the screen itself (snap_engine::agent).
            Request::Grab { target, cursor, out } => {
                let engine = self.engine.clone();
                std::thread::spawn(move || answer(reply, snap_engine::agent::grab(&engine, target, cursor, Path::new(&out))));
            }
            Request::Status => answer(reply, snap_engine::agent::status()),
        }
    }

    /// Queue `make(path)` when the file exists, else produce the error response.
    fn queue_file(&mut self, path: String, make: impl FnOnce(PathBuf) -> Command) -> Response {
        let path = PathBuf::from(path);
        if !path.is_file() {
            return Response::err(snap_core::tf("app.error.no_file", &[("path", &path.display().to_string())]));
        }
        self.cmds.push(make(path));
        Response::ok()
    }

    /// The non-interactive form of an action, when the request carries what the overlay would
    /// have asked the user for (a region, a window title, a monitor).
    fn direct_command(
        &self,
        action: Action,
        pipeline: Option<String>,
        region: Option<snap_core::Rect>,
        window_title: Option<String>,
        monitor: Option<usize>,
        reply: Option<Sender<Response>>,
    ) -> Option<Command> {
        let settings = &self.settings;
        let run = |request: CaptureRequest, work: Work| Some(Command::CaptureAndRun { request, work, reply: reply.clone() });
        let default_work = || Work::Pipeline(pipeline.clone().unwrap_or_else(|| settings.default_pipeline.clone()));
        let region_mode =
            |mode: OverlayMode| region.and_then(|r| run(CaptureRequest::Rect(r), mode_work(mode, pipeline.clone(), settings)?));
        match action {
            Action::CaptureWindow => run(CaptureRequest::WindowTitled(window_title?), default_work()),
            Action::CaptureMonitor => run(CaptureRequest::Monitor(monitor?), default_work()),
            Action::CaptureRegion => run(CaptureRequest::Rect(region?), default_work()),
            Action::OcrRegion => region_mode(OverlayMode::Ocr),
            Action::DecodeCodesRegion => region_mode(OverlayMode::Codes),
            Action::TranslateRegion => region_mode(OverlayMode::Translate),
            Action::PinRegion => run(CaptureRequest::Rect(region?), Work::Steps(vec![snap_core::pipeline::Step::Pin])),
            Action::RecordRegion | Action::RecordGif => {
                let format = if action == Action::RecordGif { snap_core::settings::VideoFormat::Gif } else { settings.recording.format };
                Some(Command::StartRecording { target: snap_engine::RecordTarget::Region(region?), format })
            }
            Action::CaptureScrolling => Some(Command::StartScrolling { rect: region?, pipeline }),
            _ => None,
        }
    }
}
