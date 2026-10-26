# apps/snapoleon: architecture

How the desktop app is put together: the window registry, the `View` contract, commands, jobs and
the dev harness. Read it together with `docs/SPEC.md` (what each view does) and `docs/DESIGN.md`
(how it looks).

Views live in `apps/snapoleon/src/views/<view>/` with their strings in `locales/{en,ru}/<view>.toml`.
The `View` trait, `ViewCx`, commands, routing, jobs, tray, hotkeys and IPC are shared by every view;
change them deliberately.

## Map

```
apps/snapoleon/
  build.rs            Windows icon, version info, manifest (assets/snapoleon.{ico,manifest})
  src/main.rs         startup: dpi, console, logging, policy, settings, i18n, CLI dispatch, shell
  src/app.rs          App: window registry, shell callbacks, lifecycle, settings re-apply
  src/routing.rs      what the app does for every Command
  src/notices.rs      notices (toast or OS notification), progress pill, end of jobs
  src/events.rs       AppEvent: everything the UI thread can be woken with
  src/commands.rs     Command: the cross-view vocabulary
  src/actions.rs      hotkey Action -> Command routing; OverlayResult -> Command routing
  src/jobs.rs         worker threads, GuiPipelineUi (the engine's PipelineUi)
  src/tray.rs         tray icon and menu       src/hotkeys.rs  global hotkeys
  src/ipc.rs          single instance, CLI/URL requests
  src/launch.rs       macOS: a start from a terminal relaunches through LaunchServices
  src/theme.rs        active theme shared by all windows
  src/logging.rs      stderr + %APPDATA%/.../logs/snapoleon.log
  src/dev.rs          dev harness (SNAPOLEON_DEV=...)
  src/views/mod.rs    View, ViewCx, OpenCx, ViewHandle, Chrome, window_spec
  src/views/countdown.rs  the count-down tile (delayed capture; the recorder's 3-2-1 draws the same one)
  src/views/<view>/   one directory per view, entered through its `open` function
```

## Threading model

- **One UI thread** runs winit, egui and every `View`. All callbacks (`ui`, `on_msg`, `closed`,
  commands) run on it, one at a time. Views are not `Send` and may hold `Rc<RefCell<..>>`.
- **Worker threads** (`std::thread`, no tokio) run everything that blocks: capture, OCR, encode,
  upload, file I/O, history queries that may be slow. They never touch egui or the shell.
- Workers reach the UI thread through `Proxy<AppEvent>` (a `Send + Sync` handle):
  `proxy.send(AppEvent::..)`. Views get a narrower `ViewHandle` that delivers a message to one
  window's `View::on_msg`.
- The shell repaints a window only on input or when asked. After a message arrives the app
  repaints the target window; for animations call `ctx.request_repaint_after(..)` yourself.
- Never block the UI thread for more than a few milliseconds. `Engine` methods block: call them
  from `cx.spawn(..)` or a job, not from `ui`.

## The View contract

```rust
pub trait View {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &egui::Context);                 // every frame of this window
    fn close_requested(&mut self, cx: &mut ViewCx) -> bool { true }         // false = keep open (unsaved changes)
    fn closed(&mut self, cx: &mut ViewCx) {}                                // window is gone; last chance to push commands
    fn on_msg(&mut self, cx: &mut ViewCx, msg: Box<dyn Any + Send>) {}      // see "Messages and background work"
}
```

`ViewCx` (passed to every call):

| field / method | meaning |
|---|---|
| `shell` | `&mut Shell<AppEvent>`: `close`, `focus`, `set_outer_rect`, `drag_window`, `monitors`, `native_id`, `set_always_on_top`, `set_passthrough`, `set_exclude_from_capture`, ... |
| `win` | this window's `WindowKey` |
| `engine` | `&Engine` (clone it to move into a thread) |
| `settings` | `&Settings` snapshot, refreshed after a settings change |
| `cmds` / `push(cmd)` | deferred commands, processed right after the current call returns |
| `toasts` | this window's toast stack; `toast(kind, text)`, `toast_action(kind, text, label, command)`, `toast_error(&err)` |
| `proxy` | `Proxy<AppEvent>` for threads |
| `handle()` | `ViewHandle` (clone, `Send`): `handle.send(msg)` -> your `on_msg` |
| `spawn(work)` | run `work` on a thread; its return value arrives as `on_msg(Box<T>)` |
| `notify(Notice)` | app-level notice: in-app toast when an app window is focused, OS notification otherwise |
| `update_settings(\|s\| ..)` | persist a settings change and make the app re-apply it |
| `close()` | close this window (does not call `close_requested`) |

### Opening windows

Each view module exposes `pub const TAG: &str` and `pub fn open(cx: &mut OpenCx, ..)`. The app
calls `open` when the matching command arrives (table below). Inside `open`:

```rust
cx.open(window_spec(t("editor.title"), TAG, (1100.0, 760.0), (720.0, 480.0)), MyView::new(..));
```

- `window_spec(title, tag, size, min_size)` gives the standard app window: the app's own title bar
  (`WindowSpec::title_bar`; on macOS the system's traffic lights, rounded corners and shadow stay around
  it, elsewhere there are no OS decorations and Windows 11 rounds the corners), resizable, app icon.
  Sizes are logical pixels; on a small screen the shell shrinks them to fit, and on macOS it centres the
  window on the screen under the pointer. Build other kinds (pins, HUDs, overlays) from `WindowSpec`
  directly; see `pin`, `recorder`, `scroll` and `overlay`.
- `WindowSpec::activate` (default `true`): the window asks for the foreground when it appears, so a window opened by
  a hotkey or the tray does not come up behind the app the user is in. Windows that sit over the user's work and
  must not take the keyboard from it (pins, the recorder and scroll HUDs, count-down tiles) set it to `false`.
- A panic inside a view is caught by the app: that window is closed (its `closed` runs), the user gets a notice and
  the app lives on. The same goes for a panic in a job thread (the job ends with an error).
- `cx.open(spec, view)` returns the `WindowKey`. A window whose `spec` is not a plain app window
  (cover, tool window, always-on-top, skip-taskbar) is not used for toasts or the progress pill.
- **Singletons** (library, settings, about, onboarding, colors, steps): start `open` with
  `if cx.focus_tagged(TAG) { return; }`. To pass data to the existing window use
  `cx.post(win, msg)` (see `settings::open` for a page change).
- **Several windows, one session** (the overlay's per-monitor covers): create one
  `Rc<RefCell<Session>>`, give a clone to each window's view, store the sibling `WindowKey`s in
  it. When the session ends, `cx.shell.close(..)` every sibling from whichever window noticed.
  See `views/overlay/mod.rs`.
- A view that needs another window while running pushes
  `Command::Open(Box::new(|cx| my_view::open_something(cx, data)))`. Build the new view *inside*
  the closure (views are not `Send`).

### The standard chrome

```rust
Chrome::new(&title).show(cx, ctx, |ui, cx| { /* body: a vertical Ui below the title bar */ });
Chrome::new(&title).center(|ui| search_field(ui, &mut q, hint)).no_maximize().show(..)
```

`Chrome` draws the title bar (`snap_ui::widgets::title_bar`: drag, minimize/maximize/close, logo,
optional center slot), the 1 px window outline, and performs the requested window actions. The
body gets the rest of the window. On macOS the window controls and the outline are the system's: the
bar keeps room for the traffic lights (`Shell::title_bar_inset`), and a double-click on it does what
the Desktop & Dock setting says. `.close_on_escape()` makes Escape close a dialog-like window (About). Use `snap_ui::widgets` and `Palette` for everything inside; never
hand-pick colors, radii or fonts (`docs/DESIGN.md`).

## Messages and background work

```rust
enum Msg { Loaded(Vec<Entry>), Saved(Result<PathBuf, String>) }          // your own type

// in ui(), on a click:
let engine = cx.engine.clone();
cx.spawn(move || Msg::Saved(engine.save(..).map_err(|e| e.to_string())));

// later:
fn on_msg(&mut self, cx: &mut ViewCx, msg: Box<dyn Any + Send>) {
    if let Ok(msg) = msg.downcast::<Msg>() { match *msg { .. } }
}
```

For streaming results (progress of a scrolling capture, frames of a recorder) clone
`cx.handle()` into the thread and call `handle.send(..)` as often as needed. Messages for a window
that no longer exists are dropped. A thread outliving its window should check a shared
`AtomicBool`/channel you close in `closed`.

## Commands and routing

Views never open each other. They push a `Command`; the app routes it. The rows are the whole
vocabulary; the "view must send back" column is what the receiving view owes the rest of the app.

| Command | goes to | the view must... |
|---|---|---|
| `StartCapture { action, pipeline }` | `actions::start_action`: overlay, immediate capture job, or the view an `Action` names | (app-internal) |
| `OpenOverlay(OverlayRequest)` | `overlay::open` | on finish push `actions::overlay_commands(result, &request, cx.settings)` and close every window of the session. `OverlayMode`: Region, Window, Element, Ocr, Codes, Translate, Pin, Record(format), Scroll, ColorPicker, Measure. `OverlayResult`: `Selected(capture)` (selection confirmed), `Action(OverlayAction, capture)` (instant-toolbar button), `Color(ColorSample)`, `Measured(Measurement)`, `Cancelled`. Only one overlay at a time (`open` returns if one exists). |
| `CaptureAndRun { request, work, reply }` | worker job: `engine.capture` then pipeline or steps | (no view) |
| `RunPipeline { capture, pipeline }` / `RunSteps { capture, steps }` | worker job: `engine.run` / `engine.run_steps` with `GuiPipelineUi` | (no view). Use `RunSteps` for editor/overlay buttons (Copy, Save, Upload, Pin...). Progress, notices and errors are shown by the app. |
| `ProcessFile { path, pipeline, reply }` | worker job: `engine.run_file` | (no view) |
| `OpenEditor(EditorInput)` | `editor::open` | when `input.reply` is `Some`, send **exactly once** on close: `Some(Annotated { image, document })` if the user finished (Done / Copy / Save), `None` if they discarded. A dropped sender counts as `None`. The pipeline's `Annotate` step blocks on this. |
| `EditFile(path)` | loads on a thread, then `OpenEditor` | (no view) |
| `OpenPin { image, origin }` | `pin::open` | `origin` is the physical screen rect of the source pixels: appear exactly over them. |
| `ClosePins` / `UnlockPins` | every window tagged `pin` | tray items: close them all / leave click-through (`on_msg(Box<pin::Unlock>)`). The tray shows them only while a pin is open. |
| `PinFile(path)` | loads on a thread, then `OpenPin` | (no view) |
| `OpenLibrary` | `library::open` (singleton) | |
| `OpenSettings { page }` | `settings::open` (singleton); an open window gets `on_msg(Box<SettingsPage>)` | switch to the page. Pages: General, Capture, Hotkeys, Pipelines, Destinations, Links, Recording, TextAi, Privacy, History, Advanced. |
| `OpenOnboarding` | `onboarding::open` (singleton) | set `general.first_run_done = true` via `cx.update_settings` when finished, and push `FirstRunDone` when that ends the very first run. |
| `FirstRunDone` | `App::first_run_hint`: a notification that the app now lives in the menu bar (the tray elsewhere) with the capture hotkey, and the menu bar icon blinks (`Tray::flash`) | (no view) |
| `OpenAbout` | `about::open` (singleton) | |
| `ShowText { title, text }` | `text::open` | |
| `ShowCodes { title, text, codes }` | `text::open_with_codes` | decoded QR codes and barcodes (`PipelineUi::show_codes`, i.e. a `DecodeCodes { copy: false }` step). |
| `StartRecording { target, format }` | `recorder::open(RecorderInput)`; refused with a notice if policy disables recording, ignored if one runs | start `snap_record`; push `Command::RecordingState { active: true }` when recording starts and `{ active: false }` when it ends (tray icon, quit handling). On stop: `engine.add_recording(..)`, then a notice or `Command::OpenVideoEditor`. |
| `StopRecording` | `on_msg(Box<RecorderMsg::Stop>)` of every recorder window | finish and keep the recording. |
| `RecordingState { active }` | tray icon + menu | sent by the recorder view only. |
| `OpenVideoEditor { path }` | `video::open` | |
| `StartScrolling { rect, pipeline }` | `scroll::open(ScrollInput)` | run `engine.scrolling(rect, &cancel, ..)` on a thread, stream `ScrollProgress` previews, then push `RunPipeline { capture, pipeline: pipeline or settings.default_pipeline }`. |
| `StartStepCapture` | `steps::open` (singleton). The step-capture hotkey while a session runs sends `on_msg(Box<StepsMsg::Toggle>)` | session HUD via `engine::steps::StepRecorder`; afterwards the guide editor. |
| `OpenCombine { images }` | `combine::open` | empty `images` = start empty. |
| `OpenDiff { a, b }` | `diff::open` | `None` leaves that side to the tool's own picker. The tray's "Compare images" sends `{ None, None }`; a view that has the two images (a two-card selection in the Library) can send them. |
| `OpenColorTool(ColorToolInput)` | `colors::open` (singleton); a sample or measurement reaches an open window as `on_msg(Box<ColorSample>)` / `Box<Measurement>` | keep a history of picks and measurements. `ColorToolInput::Open` (tray: "Colors and measurements") just shows the history. |
| `PauseHotkeys(bool)` | hotkey manager | the Settings hotkey recorder sends `true` on focus and `false` on blur. |
| `CopyText`, `Reveal`, `OpenPath`, `OpenUrl` | platform helpers (errors become a notice) | |
| `FixPermission(Perm)` | `notices::fix_permission`: the system prompt while the permission is undecided, its System Settings page after that | the button of a notice whose `permission` is set (toast and macOS notification). |
| `Open(closure)` | runs the closure with an `OpenCx` | see "Opening windows". |
| `Quit` | stops a running recording first, closes windows, exits | |

### Where actions come from

- **Hotkeys** (`settings.hotkeys`) and the **tray** (menu, left click = `settings.general.tray_click`)
  become `Command::StartCapture { action, pipeline }`.
- **IPC** (`snapoleon capture ...`, `snapoleon://...`, `snapoleon quit`, a second launch) arrives as
  `snap_core::ipc::Request`; `ipc.rs` maps each variant. A request that already carries what the
  overlay would ask (a region, a window title, a monitor) runs without UI; the CLI gets the file
  path, link or text back when the job finishes.
  A `snapoleon://` URL becomes `Request::Link { action }`, which carries nothing but the action: it runs as
  the interactive form (overlay, picker), never with a target, pipeline or path from the URL.
  `Request::Grab` (a headless CLI or MCP capture that may not read the screen itself, see
  `snap_engine::agent` and `docs/CLI.md#macos`) is answered on a worker thread with the image in a PNG file;
  `Request::Status` answers the app's version and privacy permissions for `snapoleon doctor`.
- `actions::start_action` is the single table from `Action` to flow:
  CaptureRegion/Window/Element/Scrolling -> overlay; CaptureMonitor/AllMonitors/LastRegion ->
  immediate job; CaptureDelayed -> the count-down tile (`views::countdown`), then the region overlay; RecordRegion/Gif
  -> overlay in record mode; RecordScreen -> `StartRecording` on the monitor under the cursor;
  Ocr/DecodeCodes/Translate/PinRegion/ColorPicker/Measure -> overlay modes; PinClipboard,
  OpenEditorFromClipboard -> clipboard image or a notice; StepCaptureToggle, OpenLibrary,
  OpenSettings -> their views.

## History changes

The engine tells the app whenever it adds to the history or changes an entry it added (a finished pipeline, a
recording, a new link, OCR text found by the background indexing): `Engine::set_history_listener` is wired to
`AppEvent::HistoryChanged`. The app refreshes the tray's recent list and delivers `crate::events::HistoryChanged`
(a unit struct) to **every** open window's `View::on_msg`. A view that lists the history (the Library) should
reload on it (`msg.downcast::<HistoryChanged>()`) instead of waiting for its poll. A view that writes to the history without going through the engine sends the event itself with
`cx.proxy.send(AppEvent::HistoryChanged)`.

## The overlay's covers

Opening a borderless full-screen window, its GPU surface and the first frame takes longer than the 100 ms the
overlay has to appear in. The tray app therefore opens one cover window per monitor **hidden** at start-up
(`overlay::prewarm`, after the tray is up) and keeps them (`views/overlay/pool.rs`). A session freezes the screen,
gives each cover its monitor's snapshot (a `Seat`) and calls `Shell::present`, which renders a fresh frame while the
window is still hidden and shows it with that frame. When the session ends the covers are hidden again
(`Shell::set_visible(false)` plus `Shell::refresh_hidden`, so the snapshot texture is released). A monitor layout
that differs from the pooled covers replaces them on the next session. The pool is the setting
`general.instant_overlay` (default on; about 100 MB of resident memory with the OpenGL renderer, about 200 MB with wgpu: `docs/notes/renderer.md`): turning it off closes the resting covers and
each session opens and closes its own, turning it on opens them (`overlay::set_instant`, from `SettingsChanged`). The dev harness has no pool (it must end with
its last window), and a tray-less app has none either. Use `overlay::active()` to ask whether a session is running;
`windows_with_tag(overlay::TAG)` also finds the resting covers. `overlay ready N ms after the hotkey` in the log is
the measurement (about 30 ms on a 2560x1440 monitor).

## Jobs

Engine calls run in `jobs::spawn`: a named worker thread with a `GuiPipelineUi`:

- `annotate` sends `Command::OpenEditor` with a reply channel and blocks the worker;
- `pin` -> `Command::OpenPin`; `show_text` -> `Command::ShowText`;
- `notify` -> `AppEvent::Notice`; `progress(label, frac)` -> the progress pill (bottom-left of the
  last focused app window) and the tray tooltip. An empty label clears it.

When the job ends, `AppEvent::JobDone` clears progress, answers an IPC `reply`, and reports an
error as a notice. Steps that yield text without copying it (`Ocr { copy: false }`, `Translate`)
open the text window with `Outcome.text` unless the engine already called `show_text`.
`Error::Cancelled` (the user dismissed the editor) is not an error.

If your view needs the *result* of an engine call (upload link in the editor, OCR text for the
side panel) do not use a job: `cx.spawn` it and handle the typed result in `on_msg`.

## Toasts and notices

- **View-local feedback** ("Copied", "Saved as ..."): `cx.toast(ToastKind::Success, t("editor.copied"))`.
  A toast with a button: `cx.toast_action(kind, text, label, Command::Reveal(path))`.
- **Results that matter even when no window is focused** (job results, hotkey conflicts, background
  failures): `cx.notify(Notice { kind, title, body, url, file, image, permission })`. The app shows an in-app
  toast (with "Copy link" / "Show in folder") when an app window is focused and an OS notification
  otherwise (respecting `general.show_notifications`; errors are always shown).
- **A missing privacy permission** (macOS): set `Notice::permission` to `notices::missing_permission(&err)`. The toast
  and the notification then carry "Open System Settings" (`Command::FixPermission`). Jobs, the overlay, scrolling
  capture and element snapping already do this for `Error::Permission`; while Screen Recording is off the tray menu
  starts with "Allow Screen Recording…", which opens Settings > Privacy.
- Errors: `cx.toast_error(&err)` uses the localised message for known error types.

## Settings

Read `cx.settings` (snapshot). Change with `cx.update_settings(|s| s.capture.delay_secs = 5)`:
it persists through the engine and the app re-applies theme, language, hotkeys, autostart and the
tray menu. Settings are applied immediately (no Save button, per DESIGN.md). Reduce motion
(`general.reduce_motion`) and the system light/dark switch are handled by the app, not by views.
Secrets never go in settings: use `cx.engine.secrets()`.

## File dialogs and the clipboard

`rfd` (native dialogs; the xdg-desktop-portal on Linux, so no GTK) blocks until the user answers, so
call it from a worker, never from `ui`:

```rust
cx.spawn(|| Msg::Picked(rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "webp"]).pick_file()));
```

The clipboard is `snap_platform::clipboard` (`set_image`, `set_text`, `get_image`, ...); opening and
revealing files is `snap_platform::open`. Both are safe to call from any thread; from a view,
`Command::CopyText` / `Reveal` / `OpenPath` / `OpenUrl` also report failures as a notice.

## i18n

Every user-visible string goes through `snap_core::t("view.key")` / `tf(..)`. Each view owns
`locales/en/<view>.toml` and `locales/ru/<view>.toml` (create both, same keys, natural Russian,
sentence case, no exclamation marks). Use the shared `common.*` and `action.*` keys where they fit.
Keys under `app.*` belong to the app core (tray, notices, startup).

## Dev harness

Run one view in isolation with realistic demo data:

```
SNAPOLEON_DEV=editor cargo run -p snapoleon
SNAPOLEON_DEV=settings SNAPOLEON_DEV_PAGE=hotkeys SNAPOLEON_DEV_THEME=light cargo run -p snapoleon
SNAPOLEON_DEV=library SNAPOLEON_DEV_SHOT=$TEMP/library.png cargo run -p snapoleon   # saves a screenshot and quits
```

| variable | effect |
|---|---|
| `SNAPOLEON_DEV=<view>` | `overlay editor pin recorder video library text settings onboarding about steps combine diff scroll colors`. Opens that view; no tray, hotkeys, IPC, autostart or URL scheme; the process ends with the last window. |
| `SNAPOLEON_DEV_PAGE` | settings page key: `general capture hotkeys pipelines destinations links recording text_ai privacy history advanced` |
| `SNAPOLEON_DEV_MODE` | overlay mode: `region window element ocr codes translate pin record gif scroll color measure` |
| `SNAPOLEON_DEV_VIDEO` | video file for the `video` view (default: a generated slideshow) |
| `SNAPOLEON_DEV_THEME` | `light` or `dark` |
| `SNAPOLEON_DEV_LANG` | `en` or `ru` |
| `SNAPOLEON_DEV_SHOT=<path.png>` | after `SNAPOLEON_DEV_SHOT_DELAY_MS` (default 1500) screenshot the view's first window via `snap_capture::capture_window` on its native id (falls back to cropping the screen at its outer rect), print the path, quit. Windows that hide themselves from captures (recorder and scroll HUDs) are un-hidden for the shot. On macOS this runs with the terminal's Screen Recording permission and goes through the capture code under test; screenshot the window with peekaboo instead (below). |
| `SNAPOLEON_DEV_DEMO` | `1`: demo content where a view would otherwise wait for real input: the steps HUD records a simulated session, the scroll view "scrolls" the screen below its area, the colors tool starts with sample picks. `guide`: the steps view opens the guide editor on a finished demo guide. |
| `SNAPOLEON_DEV_EDITOR_*` | editor state for screenshots (`views/editor/demo.rs`): `_DEMO=1` sample shapes, `_TOOL=<id>` active tool, `_SELECT=last\|all`, `_TAB=beautify\|text\|export`, `_PANEL=0` hides the side panel, `_BEAUTIFY=1`, `_EDIT_TEXT=1`, `_RUN=ocr\|redact`, `_OPEN=<file.snap>`, `_PII=1` a page with personal data instead of the screenshot. |
| `SNAPOLEON_DEV_SHARED_DATA` | use the real settings, history and logs instead of the private ones |
| `SNAPOLEON_RENDERER` | not a dev-only variable: `wgpu` or `glow` forces that renderer (and `glow` never falls back); `glow-fail` makes OpenGL context creation fail on purpose, to test the fallback. The log line `snap-shell renderer` says what is in use. See "Renderer" below. |

Demo data: `editor`, `pin`, `combine` and `diff` use real crops of a screenshot of the primary
monitor taken at startup; `library` seeds an empty history with 14 entries (files, thumbnails,
OCR text, tags, favorites, links) the first time; `text` shows a mixed English and Russian
sample; `settings` opens the requested page; `video` generates a slideshow MP4 when possible.

Dev runs use **portable data** (`<exe dir>/data`, which sits in your private `CARGO_TARGET_DIR`), so
they never touch the installed app's settings or history. The log is in `data/logs/snapoleon.log`.
Read your screenshots (the Read tool shows PNGs) and fix what looks off before you report.

### On macOS

`cargo run` starts the app as part of the terminal, which is fine for the harness. For capture, permissions,
notifications, hotkeys and links use the signed bundle (`packaging/macos/bundle.sh --debug --install`, see
`docs/MACOS.md`). To look at a window, use peekaboo, which reads windows without focusing them, even when covered:

```
peekaboo window list --app snapoleon --json               # ids, titles, frames
peekaboo see --window-id <id> --no-elements --path out.png
```

Window lists also show an untitled, invisible 500x500 window owned by Snapoleon at the bottom-left of the main screen.
It is not ours: it is `TUINSWindow`, the text input system's cursor accessory (input source and Caps Lock indicator),
which macOS creates for every app whose window has a text field. Leave it alone (suppressing the text input context
would break IME in text fields); `snapoleon list windows` and the MCP `list_windows` filter it out.

## Renderer

`snap-shell` draws with OpenGL (glutin + egui_glow, one context shared by all windows) on Windows and Linux and with
wgpu on macOS. Windows and Linux builds carry both: at the first window the shell tries OpenGL (on Linux desktop GL, then
OpenGL ES 3.0 and 2.0 through EGL) and, if no context can be made or it is older than OpenGL 3.0 / ES 2.0 (remote
desktop, a VM, the Basic Display Adapter, an old GPU), logs why (`OpenGL is not usable (...); falling back to wgpu`) and
uses wgpu from then on: DX12 including WARP, or Vulkan, on Windows; Vulkan, or GL through wgpu-hal, on Linux. The `wgpu`
cargo feature of `snapoleon` forces wgpu alone on every
OS. `SNAPOLEON_RENDERER=wgpu|glow` picks one at run time where both are compiled in. Measurements:
`docs/notes/renderer.md`.

Parallel pixel loops (snapshot upload, BGRA conversion, tone mapping, image bands) run on the shared `rayon` pool and
the element-query worker is one thread for the process: an OpenGL driver keeps about 90 KB for every thread ever
created, so a thread per capture showed up as memory creeping up by about 1.3 MB per overlay session. Do not spawn
threads per session on the overlay path.

## Accessibility

Every window carries an AccessKit adapter (`snap-shell`), so screen readers and UI agents (VoiceOver, peekaboo, Windows
UI Automation, AT-SPI) see the egui widgets as buttons, switches, sliders and fields. egui builds the tree only while
such a client is attached; without one it costs nothing.

A widget is found by its accessible name, so every control needs one:

- `snap_ui` widgets name themselves from their label or tooltip. Controls that show no text of their own (switches,
  sliders, dropdowns, text and number fields, shortcut recorders, color wells) take the name of the row or field that
  hosts them: `setting_row`, the settings `form_row` and `stacked`, and the views' `field` / `row` helpers wrap their
  control in `snap_ui::widgets::util::with_control_name`. Do the same for a new label-plus-control layout.
- A hand-drawn clickable (`allocate_exact_size(.., Sense::click())`) calls
  `resp.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), label))`, or `WidgetInfo::selected(..)` when
  it shows an on/off or chosen state. Passing it through `snap_ui::widgets::tooltip` names an icon-only one after its
  tooltip.
- A surface that only moves the window takes `Sense::DRAG` (or `CLICK | DRAG`): not focusable, so it is neither a Tab
  stop nor a nameless element.

Window titles (`WindowSpec::title`) are what window lists and agents show, also for windows whose title bar the app
draws: keep them meaningful.

## Startup, for reference

`main`: `dev::prepare` (dev data dir) -> `dpi::init` -> `console::attach` -> settings (they decide the language
of the help) -> `cli::parse` (localised help; a bare `snapoleon://` URL or an existing path as the first argument
means `url` / `edit`, which is how desktop entries and file associations call the app) -> logging (headless
commands: warnings on stderr only, never stdout, so `snapoleon mcp` stays clean; the app: stderr plus the log file)
-> headless subcommands run in-process; GUI ones forward to a running instance
(`ipc::instance_running`) or start the shell. On macOS a GUI start that LaunchServices did not make (the parent is not
launchd: a terminal, the PATH script) from inside an app bundle is handed to a copy started with `open`
(`launch.rs`), so the app is its own responsible process for the privacy permissions. In `App::init` (first callback, after the event
loop started): serve IPC (a lost race forwards and exits), register hotkeys (conflicts become
notices), create the tray, sync autostart, set the toast icon, purge history by retention on a
thread, make sure the URL scheme points at this executable, and on first run open onboarding; a plain non-background launch
opens the Library; `--background` (autostart) stays in the tray. Portable installs never register
autostart or the URL scheme.

On macOS a second launch (Finder, Spotlight, the Dock icon), a `snapoleon://` link and a document opened with
the app never start a second process: they reach the running app as Apple Events, which `snap-shell` turns into
`OsRequest`s (`App::handle_os_request` in `ipc.rs` gives them the IPC paths above). The app menu's About,
Settings and Quit (Cmd+Q, and logout or restart, which wait for `App::quit`) arrive the same way; its Edit
menu feeds the focused window the input of the matching shortcut. The app is an accessory (the menu bar extra
only) until an app window opens, and has a Dock icon and the app menu until the last one closes.
