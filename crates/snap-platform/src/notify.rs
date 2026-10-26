//! Desktop notifications.
//!
//! - Windows: WinRT toast. Unpackaged executables need an AppUserModelID; it is
//!   registered per user under `HKCU\Software\Classes\AppUserModelId\app.snapoleon`.
//! - macOS: `UNUserNotificationCenter` when running from the app bundle: the banner carries the app's name and icon,
//!   a preview, and buttons, and a click comes back through [`init`]'s callback. Without a bundle (`cargo run`, the
//!   dev harness) the framework aborts the process, so those runs fall back to `osascript display notification`,
//!   which is attributed to Script Editor and cannot show an image. So does a bundle the system refuses (one without
//!   a stable code signature). The first notification asks the user for permission.
//! - Linux: freedesktop notifications through `notify-rust`.

use std::path::PathBuf;

use image::RgbaImage;
use snap_core::Result;

use crate::permissions::Perm;

/// What an OS notification shows, and what it is about.
#[derive(Debug, Clone, Default)]
pub struct OsNotice {
    pub title: String,
    pub body: String,
    /// A preview picture where the platform supports one. It is never moved or changed.
    pub image: Option<PathBuf>,
    /// The file the notice is about: a click opens it, a button reveals it (macOS).
    pub file: Option<PathBuf>,
    /// A link the notice carries: buttons copy or open it (macOS).
    pub url: Option<String>,
    /// A privacy permission the notice is about: a click, or its button, leads to where it is granted (macOS).
    pub permission: Option<Perm>,
}

impl OsNotice {
    /// The text for platforms whose notifications cannot carry the link as an action: the link itself when there is
    /// one, so it can at least be read.
    fn plain_body(&self) -> &str {
        self.url.as_deref().filter(|u| !u.is_empty()).unwrap_or(&self.body)
    }
}

/// The user clicked a notification or one of its buttons (macOS only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotifyAction {
    /// The notification itself, with what it was about.
    Open {
        file: Option<PathBuf>,
        url: Option<String>,
    },
    Reveal(PathBuf),
    CopyLink(String),
    OpenLink(String),
    /// A notification about a missing permission, or its "Open System Settings" button.
    Permission(Perm),
}

/// Button titles for notifications, in the UI language.
#[derive(Debug, Clone, Default)]
pub struct ActionLabels {
    pub open: String,
    pub reveal: String,
    pub copy_link: String,
    pub open_link: String,
    /// The button of a notice about a missing permission ("Open System Settings").
    pub permission: String,
}

/// Route notification clicks to `on_action`. Call once, on the main thread, while the app finishes launching: a click
/// on a notification that launched the app is delivered right after. The callback may run on any thread.
pub fn init(on_action: impl Fn(NotifyAction) + Send + Sync + 'static) {
    #[cfg(target_os = "macos")]
    imp::init(Box::new(on_action));
    #[cfg(not(target_os = "macos"))]
    let _ = on_action;
}

/// Set (or, after a language change, replace) the titles of the notification buttons.
pub fn set_action_labels(labels: &ActionLabels) {
    #[cfg(target_os = "macos")]
    imp::set_action_labels(labels);
    #[cfg(not(target_os = "macos"))]
    let _ = labels;
}

/// Show a notification. Returns once it has been handed to the OS.
pub fn show(notice: &OsNotice) -> Result<()> {
    #[cfg(target_os = "macos")]
    if imp::bundled() {
        imp::post(notice);
        return Ok(());
    }
    imp::show(&notice.title, notice.plain_body(), notice.image.as_deref())
}

/// The user turned notifications off for the app in the OS settings (macOS; `false` while unknown and elsewhere).
pub fn blocked() -> bool {
    #[cfg(target_os = "macos")]
    return imp::blocked();
    #[cfg(not(target_os = "macos"))]
    false
}

/// Open the OS page where notifications for the app are allowed (macOS).
pub fn open_settings() -> Result<()> {
    #[cfg(target_os = "macos")]
    return crate::open::url(&format!("x-apple.systempreferences:com.apple.Notifications-Settings.extension?id={}", snap_core::APP_ID));
    #[cfg(not(target_os = "macos"))]
    Err(snap_core::Error::unsupported("notification settings"))
}

/// Use `icon` as the application icon in toasts (Windows only; no-op elsewhere,
/// where the icon comes from the installed desktop entry / app bundle).
pub fn set_app_icon(icon: &RgbaImage) -> Result<()> {
    imp::set_app_icon(icon)
}

#[cfg(windows)]
mod imp {
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    use image::RgbaImage;
    use snap_core::{APP_ID, APP_NAME, Result, paths};
    use tauri_winrt_notification::Toast;
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    use crate::util::other;

    fn aumid_key() -> String {
        format!(r"Software\Classes\AppUserModelId\{APP_ID}")
    }

    /// Make sure the AUMID exists so toasts from this unpackaged exe are accepted
    /// and show the right name.
    fn register_aumid() -> Result<()> {
        static DONE: OnceLock<()> = OnceLock::new();
        if DONE.get().is_some() {
            return Ok(());
        }
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(aumid_key())?;
        key.set_value("DisplayName", &APP_NAME)?;
        let _ = DONE.set(());
        Ok(())
    }

    pub fn set_app_icon(icon: &RgbaImage) -> Result<()> {
        register_aumid()?;
        let dir = paths::cache_dir();
        std::fs::create_dir_all(&dir)?;
        let png: PathBuf = dir.join("toast-icon.png");
        icon.save_with_format(&png, image::ImageFormat::Png)?;
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(aumid_key())?;
        key.set_value("IconUri", &png.to_string_lossy().as_ref())?;
        key.set_value("IconBackgroundColor", &"00000000")?;
        Ok(())
    }

    pub fn show(title: &str, body: &str, image: Option<&Path>) -> Result<()> {
        register_aumid()?;
        let mut toast = Toast::new(APP_ID).title(title).text1(body);
        let picture = image.filter(|p| p.is_file()).and_then(|p| std::path::absolute(p).ok());
        if let Some(p) = &picture {
            // The toast XML wants a plain absolute path without the `\\?\` prefix.
            let plain = p.to_string_lossy().trim_start_matches(r"\\?\").to_string();
            toast = toast.image(Path::new(&plain), "");
        }
        toast.show().map_err(other)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
    use std::sync::{Once, OnceLock};

    use block2::{DynBlock, RcBlock};
    use image::RgbaImage;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Bool, ProtocolObject};
    use objc2::{AllocAnyThread, define_class, msg_send};
    use objc2_foundation::{NSArray, NSBundle, NSDictionary, NSError, NSObject, NSObjectProtocol, NSSet, NSString, NSURL, ns_string};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification, UNNotificationAction,
        UNNotificationActionOptions, UNNotificationAttachment, UNNotificationCategory, UNNotificationCategoryOptions,
        UNNotificationDefaultActionIdentifier, UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
        UNNotificationSettings, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
    };
    use snap_core::{Result, paths};

    use super::{ActionLabels, NotifyAction, OsNotice};
    use crate::permissions::Perm;

    const CATEGORY_FILE: &str = "snap.file";
    const CATEGORY_LINK: &str = "snap.link";
    const CATEGORY_BOTH: &str = "snap.both";
    const CATEGORY_PERMISSION: &str = "snap.permission";
    const ACTION_OPEN: &str = "open";
    const ACTION_REVEAL: &str = "reveal";
    const ACTION_COPY_LINK: &str = "copy_link";
    const ACTION_OPEN_LINK: &str = "open_link";
    const ACTION_PERMISSION: &str = "permission";

    /// How a permission is written into a notification's `userInfo`, which must survive a relaunch of the app.
    fn permission_key(p: Perm) -> &'static str {
        match p {
            Perm::ScreenRecording => "screen_recording",
            Perm::Accessibility => "accessibility",
            Perm::InputMonitoring => "input_monitoring",
            Perm::Microphone => "microphone",
        }
    }

    fn permission_of(key: &str) -> Option<Perm> {
        Perm::ALL.into_iter().find(|p| permission_key(*p) == key)
    }

    const STATUS_UNKNOWN: u8 = 0;
    const STATUS_ALLOWED: u8 = 1;
    const STATUS_BLOCKED: u8 = 2;
    static STATUS: AtomicU8 = AtomicU8::new(STATUS_UNKNOWN);

    type OnAction = Box<dyn Fn(NotifyAction) + Send + Sync>;
    static ON_ACTION: OnceLock<OnAction> = OnceLock::new();

    /// `UNUserNotificationCenter` aborts a process that is not an app bundle with an identifier.
    pub fn bundled() -> bool {
        static BUNDLED: OnceLock<bool> = OnceLock::new();
        *BUNDLED.get_or_init(|| {
            let bundle = NSBundle::mainBundle();
            bundle.bundleIdentifier().is_some() && bundle.bundlePath().to_string().ends_with(".app")
        })
    }

    define_class!(
        // SAFETY: NSObject has no subclassing requirements and this class does not implement Drop.
        #[unsafe(super(NSObject))]
        #[name = "SnapNotificationDelegate"]
        struct Delegate;

        // SAFETY: NSObjectProtocol has no safety requirements.
        unsafe impl NSObjectProtocol for Delegate {}

        // SAFETY: the method signatures match the UNUserNotificationCenterDelegate protocol.
        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            /// Without this the banner is suppressed whenever Snapoleon is the active app, which it often still is
            /// right after the capture overlay closed.
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                done: &DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                done.call((UNNotificationPresentationOptions::Banner | UNNotificationPresentationOptions::List,));
            }

            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(&self, _center: &UNUserNotificationCenter, response: &UNNotificationResponse, done: &DynBlock<dyn Fn()>) {
                if let (Some(action), Some(on_action)) = (action_of(response), ON_ACTION.get()) {
                    on_action(action);
                }
                done.call(());
            }
        }
    );

    impl Delegate {
        fn new() -> Retained<Self> {
            let this = Self::alloc().set_ivars(());
            // SAFETY: NSObject's init has the expected signature.
            unsafe { msg_send![super(this), init] }
        }
    }

    fn action_of(response: &UNNotificationResponse) -> Option<NotifyAction> {
        let info = response.notification().request().content().userInfo();
        let get = |key: &NSString| {
            let key: &AnyObject = key;
            info.objectForKey(key).and_then(|value| value.downcast::<NSString>().ok()).map(|value| value.to_string())
        };
        let file = get(ns_string!("file")).map(PathBuf::from);
        let url = get(ns_string!("url"));
        let permission = get(ns_string!("permission")).and_then(|key| permission_of(&key));
        let id = response.actionIdentifier().to_string();
        // SAFETY: a constant NSString exported by the framework.
        let default = unsafe { UNNotificationDefaultActionIdentifier }.to_string();
        // The notice and its only button lead to the same place.
        if let Some(permission) = permission.filter(|_| id == ACTION_PERMISSION || id == default) {
            return Some(NotifyAction::Permission(permission));
        }
        match id.as_str() {
            ACTION_REVEAL => file.map(NotifyAction::Reveal),
            ACTION_COPY_LINK => url.map(NotifyAction::CopyLink),
            ACTION_OPEN_LINK => url.map(NotifyAction::OpenLink),
            ACTION_OPEN => Some(NotifyAction::Open { file, url }),
            _ if id == default => Some(NotifyAction::Open { file, url }),
            // The dismiss action (reported only to categories that ask for it) and anything unknown.
            _ => None,
        }
    }

    pub fn init(on_action: OnAction) {
        if !bundled() {
            return;
        }
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let _ = ON_ACTION.set(on_action);
            let center = UNUserNotificationCenter::currentNotificationCenter();
            let delegate = Delegate::new();
            center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            // The center holds its delegate weakly; this one serves the whole process.
            std::mem::forget(delegate);
            let read = RcBlock::new(|settings: std::ptr::NonNull<UNNotificationSettings>| {
                // SAFETY: the framework passes a valid settings object for the duration of the call.
                let status = unsafe { settings.as_ref() }.authorizationStatus();
                let status = if status == UNAuthorizationStatus::Denied {
                    STATUS_BLOCKED
                } else if status == UNAuthorizationStatus::NotDetermined {
                    STATUS_UNKNOWN
                } else {
                    STATUS_ALLOWED
                };
                STATUS.store(status, Ordering::Relaxed);
            });
            center.getNotificationSettingsWithCompletionHandler(&read);
        });
    }

    pub fn blocked() -> bool {
        STATUS.load(Ordering::Relaxed) == STATUS_BLOCKED
    }

    pub fn set_action_labels(labels: &ActionLabels) {
        if !bundled() {
            return;
        }
        let action = |id: &str, title: &str, options: UNNotificationActionOptions| {
            UNNotificationAction::actionWithIdentifier_title_options(&NSString::from_str(id), &NSString::from_str(title), options)
        };
        // Only Open needs Snapoleon in front (the editor opens); Finder and the browser activate themselves, and a copy
        // should not steal the focus at all.
        let open = action(ACTION_OPEN, &labels.open, UNNotificationActionOptions::Foreground);
        let reveal = action(ACTION_REVEAL, &labels.reveal, UNNotificationActionOptions::empty());
        let copy_link = action(ACTION_COPY_LINK, &labels.copy_link, UNNotificationActionOptions::empty());
        let open_link = action(ACTION_OPEN_LINK, &labels.open_link, UNNotificationActionOptions::empty());
        // System Settings comes to the front by itself.
        let permission = action(ACTION_PERMISSION, &labels.permission, UNNotificationActionOptions::empty());
        let category = |id: &str, actions: &[&UNNotificationAction]| {
            UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
                &NSString::from_str(id),
                &NSArray::from_slice(actions),
                &NSArray::new(),
                UNNotificationCategoryOptions::empty(),
            )
        };
        let file = category(CATEGORY_FILE, &[&open, &reveal]);
        let link = category(CATEGORY_LINK, &[&copy_link, &open_link]);
        let both = category(CATEGORY_BOTH, &[&copy_link, &open_link, &open, &reveal]);
        let permission = category(CATEGORY_PERMISSION, &[&permission]);
        UNUserNotificationCenter::currentNotificationCenter().setNotificationCategories(&NSSet::from_slice(&[
            &*file,
            &*link,
            &*both,
            &*permission,
        ]));
    }

    /// A process-unique identifier for requests and attachment copies.
    fn unique_id() -> String {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let millis = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        format!("{millis}-{}", NEXT.fetch_add(1, Ordering::Relaxed))
    }

    /// The preview as an attachment. The framework moves an attached file into its own store, and `image` may be the
    /// user's saved screenshot, so a copy in the cache is attached instead.
    fn attachment(image: &Path, id: &str) -> Option<Retained<UNNotificationAttachment>> {
        let ext = image.extension().and_then(|e| e.to_str()).unwrap_or("png");
        let dir = paths::cache_dir().join("notify");
        std::fs::create_dir_all(&dir).ok()?;
        let copy = dir.join(format!("{id}.{ext}"));
        std::fs::copy(image, &copy).ok()?;
        let url = NSURL::from_file_path(&copy)?;
        // SAFETY: no options dictionary is passed.
        match unsafe { UNNotificationAttachment::attachmentWithIdentifier_URL_options_error(ns_string!("preview"), &url, None) } {
            Ok(attachment) => Some(attachment),
            Err(e) => {
                tracing::debug!("notification preview: {}", e.localizedDescription());
                let _ = std::fs::remove_file(&copy);
                None
            }
        }
    }

    fn error_text(e: *mut NSError) -> String {
        // SAFETY: the framework passes either null or a valid error.
        unsafe { e.as_ref() }.map_or_else(String::new, |e| e.localizedDescription().to_string())
    }

    pub fn post(notice: &OsNotice) {
        let id = unique_id();
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&notice.title));
        let body = if notice.body.is_empty() { notice.url.as_deref().unwrap_or_default() } else { &notice.body };
        content.setBody(&NSString::from_str(body));
        content.setThreadIdentifier(ns_string!("captures"));
        // No sound: the app plays its own shutter click (settings.general.play_sounds).
        content.setSound(None);
        let file = notice.file.as_ref().map(|f| NSString::from_str(&f.to_string_lossy()));
        let url = notice.url.as_deref().filter(|u| !u.is_empty()).map(NSString::from_str);
        let permission = notice.permission.map(|p| NSString::from_str(permission_key(p)));
        let category = match (&file, &url) {
            _ if permission.is_some() => Some(CATEGORY_PERMISSION),
            (Some(_), Some(_)) => Some(CATEGORY_BOTH),
            (Some(_), None) => Some(CATEGORY_FILE),
            (None, Some(_)) => Some(CATEGORY_LINK),
            (None, None) => None,
        };
        if let Some(category) = category {
            content.setCategoryIdentifier(&NSString::from_str(category));
        }
        let mut keys: Vec<&NSString> = Vec::new();
        let mut values: Vec<&NSString> = Vec::new();
        if let Some(file) = &file {
            keys.push(ns_string!("file"));
            values.push(file);
        }
        if let Some(url) = &url {
            keys.push(ns_string!("url"));
            values.push(url);
        }
        if let Some(permission) = &permission {
            keys.push(ns_string!("permission"));
            values.push(permission);
        }
        let info: Retained<NSDictionary<NSString, NSString>> = NSDictionary::from_slices(&keys, &values);
        // SAFETY: string keys and string values are property-list types, which is what userInfo must hold.
        unsafe { content.setUserInfo(&Retained::cast_unchecked::<NSDictionary>(info)) };
        if let Some(attachment) = notice.image.as_deref().filter(|p| p.is_file()).and_then(|p| attachment(p, &id)) {
            content.setAttachments(&NSArray::from_retained_slice(&[attachment]));
        }
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&NSString::from_str(&id), &content, None);

        // Asking again once the user has answered returns the answer without a prompt, so the first notification is
        // what asks for permission, in context, and is delivered as soon as it is granted.
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let fallback = (notice.title.clone(), notice.plain_body().to_string());
        let then_add = RcBlock::new(move |granted: Bool, e: *mut NSError| {
            if !granted.as_bool() {
                if e.is_null() {
                    // The user said no; respect it.
                    STATUS.store(STATUS_BLOCKED, Ordering::Relaxed);
                    tracing::info!("notifications are turned off for Snapoleon");
                } else {
                    // The system refuses the app itself (a build without a stable signature): the plain banner
                    // still tells the user what happened.
                    tracing::warn!("notifications unavailable: {}", error_text(e));
                    let _ = show(&fallback.0, &fallback.1, None);
                }
                return;
            }
            STATUS.store(STATUS_ALLOWED, Ordering::Relaxed);
            let added = RcBlock::new(|e: *mut NSError| {
                if !e.is_null() {
                    tracing::warn!("notification failed: {}", error_text(e));
                }
            });
            UNUserNotificationCenter::currentNotificationCenter().addNotificationRequest_withCompletionHandler(&request, Some(&added));
        });
        center.requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound, &then_add);
    }

    pub fn set_app_icon(_icon: &RgbaImage) -> Result<()> {
        Ok(())
    }

    fn applescript_string(s: &str) -> String {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    }

    /// Unbundled runs only (see the module docs).
    pub fn show(title: &str, body: &str, _image: Option<&Path>) -> Result<()> {
        let script = format!("display notification {} with title {}", applescript_string(body), applescript_string(title));
        crate::util::spawn_detached(Command::new("osascript").arg("-e").arg(script))?;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::path::Path;

    use image::RgbaImage;
    use snap_core::{APP_NAME, Result};

    use crate::util::other;

    pub fn set_app_icon(_icon: &RgbaImage) -> Result<()> {
        Ok(())
    }

    pub fn show(title: &str, body: &str, image: Option<&Path>) -> Result<()> {
        let mut n = notify_rust::Notification::new();
        n.appname(APP_NAME).summary(title).body(body).icon("snapoleon");
        if let Some(p) = image.filter(|p| p.is_file()).and_then(|p| std::path::absolute(p).ok()) {
            n.image_path(&p.to_string_lossy());
        }
        n.show().map(|_| ()).map_err(other)
    }
}
