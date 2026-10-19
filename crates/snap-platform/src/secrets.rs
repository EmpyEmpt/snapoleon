//! Secret storage in the OS credential store: Windows Credential Manager,
//! macOS Keychain, Linux Secret Service (GNOME Keyring / KWallet) over D-Bus.
//!
//! Built on `keyring-core` with one store crate per platform. Entries use the
//! service name `Snapoleon` and the caller's key as the account name.
//! Windows limits a credential to 2560 bytes, so keep individual secrets small
//! (tokens, passwords, JSON refresh tokens fit).

use std::sync::{Arc, OnceLock};

use keyring_core::{Entry, Error as KeyringError};
use snap_core::secrets::{MemoryStore, SecretStore};
use snap_core::{APP_NAME, Error, Result};

use crate::util::other;

/// [`SecretStore`] backed by the OS credential store.
pub struct KeyringStore {
    service: &'static str,
}

impl KeyringStore {
    /// Connect to the platform credential store. Fails when none is available
    /// (for example a headless Linux session without a Secret Service).
    pub fn new() -> Result<Self> {
        init_default_store()?;
        Ok(Self { service: APP_NAME })
    }

    fn entry(&self, key: &str) -> keyring_core::Result<Entry> {
        Entry::new(self.service, key)
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, key: &str) -> Option<String> {
        match self.entry(key).and_then(|e| e.get_password()) {
            Ok(v) => Some(v),
            Err(KeyringError::NoEntry) => None,
            Err(e) => {
                tracing::warn!("reading secret {key:?}: {e}");
                None
            }
        }
    }

    fn set(&self, key: &str, value: &str) -> Result<()> {
        self.entry(key).and_then(|e| e.set_password(value)).map_err(|e| other(format_args!("storing secret {key:?}: {e}")))
    }

    fn delete(&self, key: &str) -> Result<()> {
        match self.entry(key).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(e) => Err(other(format_args!("deleting secret {key:?}: {e}"))),
        }
    }
}

/// The process-wide secret store: the OS credential store, or an in-memory store
/// (with a warning) when the platform has none. Secrets in the in-memory
/// fallback do not survive a restart.
pub fn store() -> Arc<dyn SecretStore> {
    static STORE: OnceLock<Arc<dyn SecretStore>> = OnceLock::new();
    STORE
        .get_or_init(|| match KeyringStore::new().and_then(probe) {
            Ok(store) => Arc::new(store),
            Err(e) => {
                tracing::warn!("no OS credential store ({e}); secrets will be kept in memory only");
                Arc::new(MemoryStore::default())
            }
        })
        .clone()
}

/// Make sure the backend answers (a locked or missing keyring only shows up on first use).
fn probe(store: KeyringStore) -> Result<KeyringStore> {
    match store.entry("__probe__").and_then(|e| e.get_password()) {
        Ok(_) | Err(KeyringError::NoEntry) => Ok(store),
        Err(e) => Err(Error::unsupported(format!("credential store unusable: {e}"))),
    }
}

fn init_default_store() -> Result<()> {
    static INIT: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| {
        let store = platform_store().map_err(|e| e.to_string())?;
        keyring_core::set_default_store(store);
        Ok(())
    })
    .clone()
    .map_err(Error::unsupported)
}

#[cfg(windows)]
fn platform_store() -> keyring_core::Result<Arc<keyring_core::api::CredentialStore>> {
    Ok(windows_native_keyring_store::Store::new()?)
}

#[cfg(target_os = "macos")]
fn platform_store() -> keyring_core::Result<Arc<keyring_core::api::CredentialStore>> {
    Ok(apple_native_keyring_store::keychain::Store::new()?)
}

#[cfg(target_os = "linux")]
fn platform_store() -> keyring_core::Result<Arc<keyring_core::api::CredentialStore>> {
    Ok(zbus_secret_service_keyring_store::Store::new()?)
}
