//! The macOS app as a whole: the app menu, the Apple Events that open documents and links or quit the app, the
//! activation policy, and the native chrome around a title bar the app draws itself.
//!
//! winit owns the application delegate (`WinitApplicationDelegate`) and implements none of this, so the delegate's
//! class gets the methods at run time, before `NSApplication.run`: AppKit handles the Apple Event an app was launched
//! for (open a document, open a URL) while it finishes launching, and it only looks for the delegate method then.

use std::{
    cell::{Cell, RefCell},
    ffi::CString,
    mem,
    path::PathBuf,
};

use block2::RcBlock;
use objc2::{
    MainThreadMarker,
    encode::{Encode, Encoding},
    ffi::class_addMethod,
    rc::Retained,
    runtime::{AnyClass, AnyObject, Bool, Imp, Sel},
    sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationTerminateReply, NSEvent, NSEventModifierFlags, NSMenu, NSMenuItem, NSScreen,
    NSWindow, NSWindowButton, NSWindowStyleMask,
};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSRunLoop, NSString, NSURL, NSUserDefaults, ns_string};
use snap_core::{APP_NAME, Point, Rect, t, tf};
use winit::{
    event_loop::ActiveEventLoop,
    platform::macos::WindowAttributesExtMacOS,
    window::{Window, WindowAttributes},
};

use super::{NewWindowArea, ns_window};
use crate::{OsRequest, WindowSpec, proxy::AppHook, window::EditAction};

/// Where the delegate's requests go: the event loop's proxy. Returns false once the loop is gone.
type Sink = Box<dyn Fn(AppHook) -> bool>;

thread_local! {
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
    /// Requests that came while the app was still launching (the document or URL it was launched for); `None` once
    /// [`start`] has handed them over.
    static LAUNCH: RefCell<Option<Vec<AppHook>>> = const { RefCell::new(Some(Vec::new())) };
    /// `applicationShouldTerminate:` answered "later" and AppKit waits for `replyToApplicationShouldTerminate:`.
    static TERMINATE_PENDING: Cell<bool> = const { Cell::new(false) };
    /// The content view class answers `mouseDownCanMoveWindow` with NO (see `keep_title_bar_clicks`).
    static VIEW_HOOKED: Cell<bool> = const { Cell::new(false) };
}

/// Give the app delegate the methods the shell needs. Called once, after winit set its delegate and before the loop
/// runs.
pub(crate) fn install(sink: Sink) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    SINK.with_borrow_mut(|s| *s = Some(sink));
    let app = NSApplication::sharedApplication(mtm);
    let Some(delegate) = app.delegate() else {
        tracing::warn!("no application delegate; the app menu and Apple Events stay unhandled");
        return;
    };
    let class = AsRef::<AnyObject>::as_ref(&*delegate).class();
    type OpenUrls = extern "C-unwind" fn(&AnyObject, Sel, &AnyObject, &NSArray<NSURL>);
    type Reopen = extern "C-unwind" fn(&AnyObject, Sel, &AnyObject, Bool) -> Bool;
    type ShouldTerminate = extern "C-unwind" fn(&AnyObject, Sel, &AnyObject) -> NSApplicationTerminateReply;
    type Validate = extern "C-unwind" fn(&AnyObject, Sel, &NSMenuItem) -> Bool;
    type Action = extern "C-unwind" fn(&AnyObject, Sel, Option<&AnyObject>);
    // SAFETY: only the types change; the runtime calls each function with the signature its encoding declares.
    let (open_urls, reopen, should_terminate, validate, action) = unsafe {
        (
            mem::transmute::<OpenUrls, Imp>(open_urls),
            mem::transmute::<Reopen, Imp>(should_handle_reopen),
            mem::transmute::<ShouldTerminate, Imp>(should_terminate),
            mem::transmute::<Validate, Imp>(validate_menu_item),
            mem::transmute::<Action, Imp>(menu_action),
        )
    };
    let object = || Encoding::Object;
    add_method(class, sel!(application:openURLs:), open_urls, Encoding::Void, &[object(), object()]);
    add_method(class, sel!(applicationShouldHandleReopen:hasVisibleWindows:), reopen, Bool::ENCODING, &[object(), Bool::ENCODING]);
    add_method(class, sel!(applicationShouldTerminate:), should_terminate, NSApplicationTerminateReply::ENCODING, &[object()]);
    add_method(class, sel!(validateMenuItem:), validate, Bool::ENCODING, &[object()]);
    for (sel, _) in commands() {
        add_method(class, sel, action, Encoding::Void, &[object()]);
    }
    // AppKit notes which optional methods its delegate implements when the delegate is set, so set it anew (cleared
    // first, in case setting the same object again is a no-op). Nothing runs in between that needs a delegate.
    app.setDelegate(None);
    app.setDelegate(Some(&delegate));
    // Window tabs make no sense here, and AppKit would add their items to the Window menu.
    NSWindow::setAllowsAutomaticWindowTabbing(false, mtm);
}

/// The loop runs: build the app menu and hand over what the OS sent while the app was launching.
pub(crate) fn start() -> Vec<AppHook> {
    build_menu();
    LAUNCH.with_borrow_mut(Option::take).unwrap_or_default()
}

/// Build the app menu, again after the UI language changed.
pub(crate) fn build_menu() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    let Some(delegate) = app.delegate() else { return };
    let own = Some(AsRef::<AnyObject>::as_ref(&*delegate));
    let cmd = NSEventModifierFlags::Command;
    let name = [("name", APP_NAME)];
    let sep = || NSMenuItem::separatorItem(mtm);

    let (services, services_menu) = submenu(mtm, &t("shell.menu.services"), &[]);
    let (app_item, _) = submenu(
        mtm,
        APP_NAME,
        &[
            item(mtm, &tf("shell.menu.about", &name), sel!(snapShellAbout:), "", cmd, own),
            sep(),
            item(mtm, &t("shell.menu.settings"), sel!(snapShellSettings:), ",", cmd, own),
            sep(),
            services,
            sep(),
            item(mtm, &tf("shell.menu.hide", &name), sel!(hide:), "h", cmd, None),
            item(mtm, &t("shell.menu.hide_others"), sel!(hideOtherApplications:), "h", cmd | NSEventModifierFlags::Option, None),
            item(mtm, &t("shell.menu.show_all"), sel!(unhideAllApplications:), "", cmd, None),
            sep(),
            item(mtm, &tf("shell.menu.quit", &name), sel!(snapShellQuit:), "q", cmd, own),
        ],
    );
    let (file_item, _) = submenu(mtm, &t("shell.menu.file"), &[item(mtm, &t("shell.menu.close"), sel!(snapShellClose:), "w", cmd, own)]);
    // The Edit actions travel the responder chain: a system text field (a file dialog) takes them first, and the app
    // delegate, last in the chain, passes them to the egui window with the focus.
    let (edit_item, _) = submenu(
        mtm,
        &t("shell.menu.edit"),
        &[
            item(mtm, &t("shell.menu.undo"), sel!(undo:), "z", cmd, None),
            item(mtm, &t("shell.menu.redo"), sel!(redo:), "z", cmd | NSEventModifierFlags::Shift, None),
            sep(),
            item(mtm, &t("shell.menu.cut"), sel!(cut:), "x", cmd, None),
            item(mtm, &t("shell.menu.copy"), sel!(copy:), "c", cmd, None),
            item(mtm, &t("shell.menu.paste"), sel!(paste:), "v", cmd, None),
            item(mtm, &t("shell.menu.select_all"), sel!(selectAll:), "a", cmd, None),
        ],
    );
    let (window_item, window_menu) = submenu(
        mtm,
        &t("shell.menu.window"),
        &[
            item(mtm, &t("shell.menu.minimize"), sel!(snapShellMinimize:), "m", cmd, own),
            item(mtm, &t("shell.menu.zoom"), sel!(snapShellZoom:), "", cmd, own),
            sep(),
            item(mtm, &t("shell.menu.bring_all_to_front"), sel!(arrangeInFront:), "", cmd, None),
        ],
    );

    let bar = NSMenu::new(mtm);
    for top in [app_item, file_item, edit_item, window_item] {
        bar.addItem(&top);
    }
    app.setServicesMenu(Some(&services_menu));
    app.setWindowsMenu(Some(&window_menu));
    app.setMainMenu(Some(&bar));
}

/// Regular (Dock icon, app switcher, menu bar) or accessory (the menu bar extra only).
pub(crate) fn set_regular(regular: bool) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let policy = if regular { NSApplicationActivationPolicy::Regular } else { NSApplicationActivationPolicy::Accessory };
    if !NSApplication::sharedApplication(mtm).setActivationPolicy(policy) {
        tracing::warn!(regular, "the activation policy was not changed");
    }
}

/// When AppKit waits for the answer to the OS's request to quit (logout, restart, Quit in the Dock), give it: AppKit
/// then ends the process. False when no such request is pending.
pub(crate) fn allow_terminate() -> bool {
    if !TERMINATE_PENDING.replace(false) {
        return false;
    }
    // Not from inside this callback: answering makes AppKit tell winit that the app terminates, and winit must not be
    // re-entered while it runs the app's callback. The main run loop runs the block next, in any mode.
    let reply = RcBlock::new(|| {
        if let Some(mtm) = MainThreadMarker::new() {
            NSApplication::sharedApplication(mtm).replyToApplicationShouldTerminate(true);
        }
    });
    // SAFETY: the block runs on the main thread, where it was made.
    unsafe { NSRunLoop::mainRunLoop().performBlock(&reply) };
    true
}

/// A window whose title bar the app draws is still a titled window here: rounded corners, shadow, the traffic lights,
/// full screen and tiling come from the system. The title bar is transparent over full-size content.
pub(crate) fn chrome_attributes(attrs: WindowAttributes, spec: &WindowSpec) -> WindowAttributes {
    if spec.title_bar.is_none() || spec.cover.is_some() {
        return attrs;
    }
    attrs.with_decorations(true).with_titlebar_transparent(true).with_title_hidden(true).with_fullsize_content_view(true)
}

/// Centre the traffic lights in the app's title bar, `height` points tall, with the same margin on the left, and
/// return the width they take from the left edge (0 in full screen, where they are not in the window). AppKit lays
/// them out for its own, shorter title bar on every resize, so this runs after each one and on every frame, where it
/// only checks.
pub(crate) fn layout_title_bar(window: &Window, height: f32) -> Option<f32> {
    let ns = ns_window(window)?;
    keep_title_bar_clicks(&ns);
    if ns.styleMask().contains(NSWindowStyleMask::FullScreen) {
        return Some(0.0);
    }
    let close = ns.standardWindowButton(NSWindowButton::CloseButton)?;
    let minimize = ns.standardWindowButton(NSWindowButton::MiniaturizeButton)?;
    let zoom = ns.standardWindowButton(NSWindowButton::ZoomButton)?;
    // The buttons sit in AppKit's NSTitlebarView, inside the NSTitlebarContainerView at the top of the frame view.
    // SAFETY: view hierarchy queries on the main thread.
    let container = unsafe { close.superview()?.superview()? };
    let height = f64::from(height);
    let size = close.frame().size;
    let margin = ((height - size.height) / 2.0).round();
    let step = minimize.frame().origin.x - close.frame().origin.x;

    let mut frame = container.frame();
    let top = ns.frame().size.height - height;
    if frame.size.height != height || frame.origin.y != top {
        frame.size.height = height;
        frame.origin.y = top;
        container.setFrame(frame);
    }
    for (i, button) in [&close, &minimize, &zoom].into_iter().enumerate() {
        // The title bar view is not flipped: y counts from its bottom, which centres the button all the same.
        let origin = NSPoint::new(margin + step * i as f64, margin);
        if button.frame().origin != origin {
            button.setFrameOrigin(origin);
        }
    }
    Some((margin + step * 2.0 + size.width) as f32)
}

/// What the system does when its own title bar is double-clicked (System Settings, Desktop & Dock): zoom, fill the
/// screen, minimize or nothing.
pub(crate) fn title_bar_double_click(window: &Window) {
    let Some(ns) = ns_window(window) else { return };
    let defaults = NSUserDefaults::standardUserDefaults();
    let minimize = match defaults.stringForKey(ns_string!("AppleActionOnDoubleClick")).map(|s| s.to_string()).as_deref() {
        Some("None") => return,
        Some(action) => action == "Minimize",
        // Older systems only had a yes/no "minimize" setting.
        None => defaults.boolForKey(ns_string!("AppleMiniaturizeOnDoubleClick")),
    };
    if minimize {
        if can_minimize(&ns) {
            ns.miniaturize(None);
        }
    } else if can_zoom(&ns) {
        // "Maximize" (Zoom) and "Fill" (macOS 15): zooming a window that names no preferred size fills the screen.
        ns.zoom(None);
    }
}

/// The free area (without the menu bar and the Dock) of the screen under the pointer, or of the one containing `at`,
/// in winit's logical coordinates: points from the top left of the main screen.
pub(crate) fn new_window_area(_el: &ActiveEventLoop, at: Option<Point>) -> Option<NewWindowArea> {
    let (visible, _) = visible_frame(at)?;
    Some(NewWindowArea { x: visible.origin.x, y: visible.origin.y, w: visible.size.width, h: visible.size.height, centre: at.is_none() })
}

/// [`new_window_area`] of the screen containing `at`, in physical pixels as winit reports monitors (points times the
/// screen's scale).
pub(crate) fn work_area(_el: &ActiveEventLoop, at: Point) -> Option<Rect> {
    let (visible, scale) = visible_frame(Some(at))?;
    let px = |v: f64| (v * scale).round();
    Some(Rect::new(px(visible.origin.x) as i32, px(visible.origin.y) as i32, px(visible.size.width) as u32, px(visible.size.height) as u32))
}

/// The visible frame of a screen, flipped to a top-left origin, and the screen's scale.
fn visible_frame(at: Option<Point>) -> Option<(NSRect, f64)> {
    let mtm = MainThreadMarker::new()?;
    let screens = NSScreen::screens(mtm);
    // Cocoa's origin is the bottom left of the first screen, the one with the menu bar.
    let main_height = screens.firstObject()?.frame().size.height;
    let flip = |r: NSRect| NSRect::new(NSPoint::new(r.origin.x, main_height - r.origin.y - r.size.height), r.size);
    let found = match at {
        None => {
            let pointer = NSEvent::mouseLocation();
            screens.iter().find(|s| contains(s.frame(), pointer))
        }
        Some(p) => screens.iter().find(|s| {
            let scale = s.backingScaleFactor();
            contains(flip(s.frame()), NSPoint::new(f64::from(p.x) / scale, f64::from(p.y) / scale))
        }),
    };
    let screen = found.or_else(|| NSScreen::mainScreen(mtm))?;
    Some((flip(screen.visibleFrame()), screen.backingScaleFactor()))
}

fn contains(r: NSRect, p: NSPoint) -> bool {
    p.x >= r.origin.x && p.x <= r.origin.x + r.size.width && p.y >= r.origin.y && p.y <= r.origin.y + r.size.height
}

/// Clicks on the app's title bar must reach it. A view that says the mouse may move the window has its clicks in the
/// title bar area taken for a window drag (and a double-click for a zoom) before the app sees them, which would
/// leave the controls the app puts there (a search field) dead and zoom twice. The app's title bar starts drags
/// itself. Added once to the content view's class, winit's.
fn keep_title_bar_clicks(ns: &NSWindow) {
    if VIEW_HOOKED.get() {
        return;
    }
    let Some(view) = ns.contentView() else { return };
    VIEW_HOOKED.set(true);
    type Query = extern "C-unwind" fn(&AnyObject, Sel) -> Bool;
    // SAFETY: only the type changes; the runtime calls it with the signature its encoding declares.
    let imp = unsafe { mem::transmute::<Query, Imp>(no) };
    add_method(view.class(), sel!(mouseDownCanMoveWindow), imp, Bool::ENCODING, &[]);
}

extern "C-unwind" fn no(_this: &AnyObject, _cmd: Sel) -> Bool {
    Bool::NO
}

/// Add a method to a class at run time. `ret` and `args` are its Objective-C types, without `self` and `_cmd`.
fn add_method(class: &AnyClass, sel: Sel, imp: Imp, ret: Encoding, args: &[Encoding]) {
    let types: String = [ret, Encoding::Object, Encoding::Sel].iter().chain(args).map(ToString::to_string).collect();
    let Ok(types) = CString::new(types) else { return };
    // SAFETY: `imp` has the C signature `types` describes, the class is registered with the runtime, and methods are
    // added on the main thread before AppKit calls them.
    let added = unsafe { class_addMethod(std::ptr::from_ref(class).cast_mut(), sel, imp, types.as_ptr()) };
    if !added.as_bool() {
        tracing::warn!("{:?} already implements {:?}; left as it is", class.name(), sel.name());
    }
}

/// Hand a request to the loop, or keep it for [`start`] while the app is launching. False once the loop is gone.
fn deliver(hook: AppHook) -> bool {
    LAUNCH.with_borrow_mut(|launch| match launch {
        Some(queue) => {
            queue.push(hook);
            true
        }
        None => SINK.with_borrow(|sink| sink.as_ref().is_some_and(|send| send(hook))),
    })
}

fn launching() -> bool {
    LAUNCH.with_borrow(Option::is_some)
}

/// `application:openURLs:`: documents opened with the app (file URLs) and links of its scheme, at launch or later.
extern "C-unwind" fn open_urls(_this: &AnyObject, _cmd: Sel, _app: &AnyObject, urls: &NSArray<NSURL>) {
    for url in urls.iter() {
        let request = if url.isFileURL() {
            url.path().map(|path| OsRequest::OpenFile(PathBuf::from(path.to_string())))
        } else {
            url.absoluteString().map(|url| OsRequest::OpenUrl(url.to_string()))
        };
        if let Some(request) = request {
            deliver(AppHook::Request(request));
        }
    }
}

/// `applicationShouldHandleReopen:hasVisibleWindows:`: the app was opened again (Finder, Spotlight, the Dock icon).
extern "C-unwind" fn should_handle_reopen(_this: &AnyObject, _cmd: Sel, _app: &AnyObject, _visible: Bool) -> Bool {
    deliver(AppHook::Request(OsRequest::Reopen));
    // The app shows what it wants; AppKit's own handling would only bring a minimized window back.
    Bool::NO
}

/// `applicationShouldTerminate:`: logout, restart or Quit in the Dock. The app winds down first (a running recording
/// is saved) and ends with `Shell::quit`, which gives the answer ([`allow_terminate`]).
extern "C-unwind" fn should_terminate(_this: &AnyObject, _cmd: Sel, _app: &AnyObject) -> NSApplicationTerminateReply {
    if launching() || !deliver(AppHook::Request(OsRequest::Quit)) {
        return NSApplicationTerminateReply::TerminateNow;
    }
    TERMINATE_PENDING.set(true);
    NSApplicationTerminateReply::TerminateLater
}

/// What the app delegate does for a menu item.
#[derive(Clone, Copy)]
enum Command {
    Request(fn() -> OsRequest),
    Close,
    Minimize,
    Zoom,
    Edit(EditAction),
}

/// Every menu action the app delegate implements. The shell's own reach it as the items' target; the Edit ones
/// through the responder chain.
fn commands() -> [(Sel, Command); 12] {
    [
        (sel!(snapShellAbout:), Command::Request(|| OsRequest::About)),
        (sel!(snapShellSettings:), Command::Request(|| OsRequest::Settings)),
        (sel!(snapShellQuit:), Command::Request(|| OsRequest::Quit)),
        (sel!(snapShellClose:), Command::Close),
        (sel!(snapShellMinimize:), Command::Minimize),
        (sel!(snapShellZoom:), Command::Zoom),
        (sel!(undo:), Command::Edit(EditAction::Undo)),
        (sel!(redo:), Command::Edit(EditAction::Redo)),
        (sel!(cut:), Command::Edit(EditAction::Cut)),
        (sel!(copy:), Command::Edit(EditAction::Copy)),
        (sel!(paste:), Command::Edit(EditAction::Paste)),
        (sel!(selectAll:), Command::Edit(EditAction::SelectAll)),
    ]
}

fn command(action: Sel) -> Option<Command> {
    commands().into_iter().find(|(sel, _)| *sel == action).map(|(_, command)| command)
}

fn key_window() -> Option<Retained<NSWindow>> {
    NSApplication::sharedApplication(MainThreadMarker::new()?).keyWindow()
}

/// Close, Minimize and Zoom act on titled windows only: the app's windows with system controls, or a system panel.
/// The borderless overlays, pins and HUDs have no such controls and keep their own ways out.
fn can_close(w: &NSWindow) -> bool {
    w.styleMask().contains(NSWindowStyleMask::Titled | NSWindowStyleMask::Closable)
}

fn can_minimize(w: &NSWindow) -> bool {
    let mask = w.styleMask();
    mask.contains(NSWindowStyleMask::Titled | NSWindowStyleMask::Miniaturizable) && !mask.contains(NSWindowStyleMask::FullScreen)
}

fn can_zoom(w: &NSWindow) -> bool {
    let mask = w.styleMask();
    mask.contains(NSWindowStyleMask::Titled)
        && !mask.contains(NSWindowStyleMask::FullScreen)
        && w.standardWindowButton(NSWindowButton::ZoomButton).is_some_and(|zoom| zoom.isEnabled())
}

/// The action of every item in [`commands`].
extern "C-unwind" fn menu_action(_this: &AnyObject, cmd: Sel, _sender: Option<&AnyObject>) {
    match command(cmd) {
        Some(Command::Request(request)) => {
            deliver(AppHook::Request(request()));
        }
        Some(Command::Edit(action)) => {
            deliver(AppHook::Edit(action));
        }
        // Through the window's own buttons, so the app hears about a close as it does from the red one.
        Some(Command::Close) => {
            if let Some(w) = key_window().filter(|w| can_close(w)) {
                w.performClose(None);
            }
        }
        Some(Command::Minimize) => {
            if let Some(w) = key_window().filter(|w| can_minimize(w)) {
                w.performMiniaturize(None);
            }
        }
        Some(Command::Zoom) => {
            if let Some(w) = key_window().filter(|w| can_zoom(w)) {
                w.performZoom(None);
            }
        }
        None => {}
    }
}

/// `validateMenuItem:`: Close, Minimize and Zoom only for a key window that has the control; Edit only with a key
/// window to send it to.
extern "C-unwind" fn validate_menu_item(_this: &AnyObject, _cmd: Sel, item: &NSMenuItem) -> Bool {
    let window = key_window();
    let window = window.as_deref();
    let enabled = match item.action().and_then(command) {
        Some(Command::Close) => window.is_some_and(can_close),
        Some(Command::Minimize) => window.is_some_and(can_minimize),
        Some(Command::Zoom) => window.is_some_and(can_zoom),
        Some(Command::Edit(_)) => window.is_some(),
        Some(Command::Request(_)) | None => true,
    };
    Bool::new(enabled)
}

fn item(
    mtm: MainThreadMarker,
    title: &str,
    action: Sel,
    key: &str,
    modifiers: NSEventModifierFlags,
    target: Option<&AnyObject>,
) -> Retained<NSMenuItem> {
    // SAFETY: every action takes the sender as its one object argument, as menu actions do.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(mtm.alloc(), &NSString::from_str(title), Some(action), &NSString::from_str(key))
    };
    item.setKeyEquivalentModifierMask(modifiers);
    // SAFETY: the target is the app delegate, which lives as long as the app.
    unsafe { item.setTarget(target) };
    item
}

/// A submenu and the item that holds it in its parent menu.
fn submenu(mtm: MainThreadMarker, title: &str, items: &[Retained<NSMenuItem>]) -> (Retained<NSMenuItem>, Retained<NSMenu>) {
    let title = NSString::from_str(title);
    let menu = NSMenu::initWithTitle(mtm.alloc(), &title);
    for item in items {
        menu.addItem(item);
    }
    let holder = NSMenuItem::new(mtm);
    holder.setTitle(&title);
    holder.setSubmenu(Some(&menu));
    (holder, menu)
}
