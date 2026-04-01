use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::config::TerminalSettings;

pub const DEFAULT_SSH_PORT: u16 = 22;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionBackend {
    #[default]
    SystemOpenSsh,
    WezTermSsh,
}

impl ConnectionBackend {
    pub fn label(self) -> &'static str {
        match self {
            Self::SystemOpenSsh => "System OpenSSH",
            Self::WezTermSsh => "WezTerm SSH",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectionFolder {
    pub id: Uuid,
    pub name: String,
}

impl ConnectionFolder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectionProfile {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub folder_id: Option<Uuid>,
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    #[serde(default)]
    pub user: String,
    // TODO: Passwords are stored as plaintext in the JSON config file.
    // Future improvement: integrate with OS credential store (Windows Credential
    // Manager / macOS Keychain / libsecret) or encrypt at rest.
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub identity_file: String,
    #[serde(default)]
    pub remote_command: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub backend: ConnectionBackend,
    #[serde(default = "default_accept_new_host")]
    pub accept_new_host: bool,
    #[serde(default, skip_serializing_if = "TerminalSettings::is_empty")]
    pub terminal: TerminalSettings,
}

impl ConnectionProfile {
    pub fn new(name: impl Into<String>, host: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            folder_id: None,
            host: host.into(),
            port: DEFAULT_SSH_PORT,
            user: String::new(),
            password: String::new(),
            identity_file: String::new(),
            remote_command: String::new(),
            note: String::new(),
            backend: ConnectionBackend::SystemOpenSsh,
            accept_new_host: true,
            terminal: TerminalSettings::default(),
        }
    }

    pub fn destination(&self) -> String {
        if self.user.trim().is_empty() {
            self.host.clone()
        } else {
            format!("{}@{}", self.user.trim(), self.host.trim())
        }
    }

    pub fn host_label(&self) -> String {
        format!("{}:{}", self.host.trim(), self.port)
    }

    pub fn normalize(&mut self) {
        self.name = self.name.trim().to_string();
        self.host = self.host.trim().to_string();
        self.user = self.user.trim().to_string();
        self.password = self.password.trim().to_string();
        self.identity_file = self.identity_file.trim().to_string();
        self.remote_command = self.remote_command.trim().to_string();
        self.note = self.note.trim().to_string();
        if self.port == 0 {
            self.port = DEFAULT_SSH_PORT;
        }
    }
}

impl Default for ConnectionProfile {
    fn default() -> Self {
        Self::new("New connection", "")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConnectionStore {
    #[serde(default)]
    pub folders: Vec<ConnectionFolder>,
    #[serde(default)]
    pub connections: Vec<ConnectionProfile>,
}

impl ConnectionStore {
    pub fn normalize(&mut self) {
        for folder in &mut self.folders {
            folder.name = folder.name.trim().to_string();
        }
        self.folders.retain(|folder| !folder.name.is_empty());
        self.folders
            .sort_by_key(|folder| folder.name.to_lowercase());

        for connection in &mut self.connections {
            connection.normalize();
        }
        self.connections
            .retain(|connection| !connection.name.is_empty() || !connection.host.is_empty());
        let folder_names = self.folder_name_map();
        self.connections.sort_by_key(|connection| {
            (
                connection
                    .folder_id
                    .and_then(|folder_id| folder_names.get(&folder_id))
                    .cloned()
                    .unwrap_or_default()
                    .to_lowercase(),
                connection.name.to_lowercase(),
                connection.host.to_lowercase(),
            )
        });

        self.cleanup_unused_folders();
    }

    pub fn cleanup_unused_folders(&mut self) {
        self.folders.retain(|folder| {
            self.connections
                .iter()
                .any(|connection| connection.folder_id == Some(folder.id))
        });
    }

    pub fn folder_name(&self, folder_id: Option<Uuid>) -> Option<&str> {
        let folder_id = folder_id?;
        self.folders
            .iter()
            .find(|folder| folder.id == folder_id)
            .map(|folder| folder.name.as_str())
    }

    pub fn ensure_folder_named(&mut self, name: &str) -> Option<Uuid> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }

        if let Some(existing) = self
            .folders
            .iter()
            .find(|folder| folder.name.eq_ignore_ascii_case(name))
        {
            return Some(existing.id);
        }

        let folder = ConnectionFolder::new(name);
        let id = folder.id;
        self.folders.push(folder);
        self.folders
            .sort_by_key(|folder| folder.name.to_lowercase());
        Some(id)
    }

    pub fn connection(&self, id: Uuid) -> Option<&ConnectionProfile> {
        self.connections
            .iter()
            .find(|connection| connection.id == id)
    }

    pub fn upsert(&mut self, mut profile: ConnectionProfile) {
        profile.normalize();

        if let Some(existing) = self
            .connections
            .iter_mut()
            .find(|connection| connection.id == profile.id)
        {
            *existing = profile;
        } else {
            self.connections.push(profile);
        }

        self.normalize();
    }

    pub fn remove(&mut self, id: Uuid) -> Option<ConnectionProfile> {
        let index = self
            .connections
            .iter()
            .position(|connection| connection.id == id)?;
        let removed = self.connections.remove(index);
        self.cleanup_unused_folders();
        Some(removed)
    }

    pub fn sorted_connections(&self) -> Vec<&ConnectionProfile> {
        let mut items = self.connections.iter().collect::<Vec<_>>();
        let folder_names = self.folder_name_map();
        items.sort_by_key(|connection| {
            (
                connection
                    .folder_id
                    .and_then(|folder_id| folder_names.get(&folder_id))
                    .cloned()
                    .unwrap_or_default()
                    .to_lowercase(),
                connection.name.to_lowercase(),
                connection.host.to_lowercase(),
            )
        });
        items
    }

    fn folder_name_map(&self) -> HashMap<Uuid, String> {
        self.folders
            .iter()
            .map(|folder| (folder.id, folder.name.clone()))
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct ConnectionRepository {
    path: PathBuf,
}

impl Default for ConnectionRepository {
    fn default() -> Self {
        Self::new(default_repository_path())
    }
}

impl ConnectionRepository {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<ConnectionStore> {
        if !self.path.exists() {
            let store = ConnectionStore::default();
            self.save(&store)?;
            return Ok(store);
        }

        let data = std::fs::read_to_string(&self.path)
            .with_context(|| format!("failed to read {}", self.path.display()))?;
        let mut store: ConnectionStore = serde_json::from_str(&data)
            .with_context(|| format!("failed to parse {}", self.path.display()))?;
        store.normalize();
        Ok(store)
    }

    pub fn save(&self, store: &ConnectionStore) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create repository directory {}", parent.display())
            })?;
        }

        let mut store = store.clone();
        store.normalize();
        let data = serde_json::to_string_pretty(&store).context("failed to encode JSON")?;
        std::fs::write(&self.path, data)
            .with_context(|| format!("failed to write {}", self.path.display()))?;
        Ok(())
    }
}

fn default_repository_path() -> PathBuf {
    let base = dirs::config_local_dir()
        .or_else(dirs::config_dir)
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    base.join("rshell").join("connections.json")
}

const fn default_ssh_port() -> u16 {
    DEFAULT_SSH_PORT
}

const fn default_accept_new_host() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_roundtrip_preserves_connections() {
        let path = std::env::temp_dir().join(format!("rshell-test-{}.json", Uuid::new_v4()));
        let repository = ConnectionRepository::new(&path);

        let mut store = ConnectionStore::default();
        let folder_id = store.ensure_folder_named("Production");
        let mut profile = ConnectionProfile::new("Edge Node", "192.168.1.10");
        profile.folder_id = folder_id;
        profile.user = "deploy".into();
        profile.backend = ConnectionBackend::WezTermSsh;
        store.upsert(profile.clone());

        repository.save(&store).unwrap();
        let loaded = repository.load().unwrap();

        assert_eq!(loaded.connections.len(), 1);
        assert_eq!(loaded.connections[0].name, profile.name);
        assert_eq!(loaded.connections[0].backend, ConnectionBackend::WezTermSsh);
        assert_eq!(
            loaded.folder_name(loaded.connections[0].folder_id),
            Some("Production")
        );

        let _ = std::fs::remove_file(path);
    }
}
