//! Secret storage contract. `snap-platform::secrets::KeyringStore` is the real
//! implementation (Windows Credential Manager / DPAPI, macOS Keychain, Secret
//! Service); [`MemoryStore`] serves tests and headless one-shots.

use std::collections::HashMap;

use parking_lot::Mutex;

use crate::Result;

pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&self, key: &str, value: &str) -> Result<()>;
    fn delete(&self, key: &str) -> Result<()>;
}

#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl SecretStore for MemoryStore {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().get(key).cloned()
    }
    fn set(&self, key: &str, value: &str) -> Result<()> {
        self.0.lock().insert(key.into(), value.into());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<()> {
        self.0.lock().remove(key);
        Ok(())
    }
}

/// Conventional keyring key for a destination / service secret.
pub fn secret_key(kind: &str, id: &str) -> String {
    format!("{kind}:{id}")
}
