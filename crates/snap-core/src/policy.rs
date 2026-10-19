//! Managed (IT-admin) policy. Sources, first found wins per field:
//! - Windows: `HKLM\SOFTWARE\Policies\Snapoleon` then `HKCU\...` (see `packaging/windows/admx`).
//!   Read by `snap-platform::policy::read_windows_policy` and handed in via [`Policy::from_pairs`].
//! - macOS: managed preferences `/Library/Managed Preferences/app.snapoleon.plist` (via platform crate).
//! - All platforms: `policy.toml` in `/etc/snapoleon/`, `%ProgramData%\Snapoleon\`,
//!   or `/Library/Application Support/Snapoleon/`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::settings::{Destination, Settings};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Policy {
    /// Force this destination list; users cannot add others.
    pub destinations: Option<Vec<Destination>>,
    pub default_destination: Option<String>,
    pub disable_uploads: bool,
    pub disable_recording: bool,
    pub disable_ai: bool,
    pub disable_history: bool,
    /// Always strip metadata and force auto-redaction before upload.
    pub force_redact_before_upload: bool,
    pub retention_days: Option<u32>,
    pub language: Option<String>,
}

impl Policy {
    pub fn load() -> Policy {
        let mut p = policy_file_paths()
            .into_iter()
            .find_map(|path| std::fs::read_to_string(path).ok())
            .and_then(|s| toml::from_str::<Policy>(&s).map_err(|e| tracing::warn!("policy.toml invalid: {e}")).ok())
            .unwrap_or_default();
        if let Some(extra) = PLATFORM_POLICY.get() {
            p.overlay(extra);
        }
        p
    }

    /// Key/value pairs as stored in the registry / managed plist.
    pub fn from_pairs(pairs: &[(String, String)]) -> Policy {
        let mut p = Policy::default();
        let truthy = |v: &str| matches!(v, "1" | "true" | "True" | "yes");
        for (k, v) in pairs {
            match k.as_str() {
                "DisableUploads" => p.disable_uploads = truthy(v),
                "DisableRecording" => p.disable_recording = truthy(v),
                "DisableAI" => p.disable_ai = truthy(v),
                "DisableHistory" => p.disable_history = truthy(v),
                "ForceRedactBeforeUpload" => p.force_redact_before_upload = truthy(v),
                "RetentionDays" => p.retention_days = v.parse().ok(),
                "Language" => p.language = Some(v.clone()),
                "DefaultDestination" => p.default_destination = Some(v.clone()),
                "DestinationsToml" => {
                    #[derive(Deserialize)]
                    struct D {
                        destinations: Vec<Destination>,
                    }
                    p.destinations = toml::from_str::<D>(v).ok().map(|d| d.destinations);
                }
                _ => {}
            }
        }
        p
    }

    fn overlay(&mut self, o: &Policy) {
        if o.destinations.is_some() {
            self.destinations = o.destinations.clone();
        }
        if o.default_destination.is_some() {
            self.default_destination = o.default_destination.clone();
        }
        self.disable_uploads |= o.disable_uploads;
        self.disable_recording |= o.disable_recording;
        self.disable_ai |= o.disable_ai;
        self.disable_history |= o.disable_history;
        self.force_redact_before_upload |= o.force_redact_before_upload;
        if o.retention_days.is_some() {
            self.retention_days = o.retention_days;
        }
        if o.language.is_some() {
            self.language = o.language.clone();
        }
    }

    pub fn apply(&self, s: &mut Settings) {
        if let Some(d) = &self.destinations {
            s.destinations = d.clone();
        }
        if let Some(d) = &self.default_destination {
            s.default_destination = Some(d.clone());
        }
        if self.disable_uploads {
            s.destinations.clear();
            s.default_destination = None;
        }
        if self.disable_ai {
            s.ai.enabled = false;
        }
        if self.disable_history {
            s.history.enabled = false;
        }
        if self.force_redact_before_upload {
            s.privacy.strip_metadata = true;
        }
        if let Some(r) = self.retention_days {
            s.history.retention_days = r;
        }
        if let Some(l) = &self.language {
            s.general.language = l.clone();
        }
    }

    pub fn is_managed(&self) -> bool {
        *self != Policy::default()
    }
}

/// Set once at startup by the platform layer with registry / plist values.
pub static PLATFORM_POLICY: std::sync::OnceLock<Policy> = std::sync::OnceLock::new();

fn policy_file_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if cfg!(windows) {
        let pd = std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into());
        v.push(PathBuf::from(pd).join("Snapoleon").join("policy.toml"));
    } else if cfg!(target_os = "macos") {
        v.push(PathBuf::from("/Library/Application Support/Snapoleon/policy.toml"));
    } else {
        v.push(PathBuf::from("/etc/snapoleon/policy.toml"));
    }
    v
}
