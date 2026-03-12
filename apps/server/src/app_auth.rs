use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppAuthState {
    #[serde(default)]
    pub pending_pairings: Vec<AppPairingRequest>,
    #[serde(default)]
    pub devices: Vec<AppDeviceRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppPairingStatus {
    Pending,
    Approved,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppPairingRequest {
    pub pairing_id: String,
    pub code: String,
    pub device_label: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub status: AppPairingStatus,
    #[serde(default)]
    pub approved_device_id: Option<String>,
    #[serde(default)]
    pub claimed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppDeviceRecord {
    pub device_id: String,
    pub label: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub last_seen_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub revoked_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub token_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppPairingPollStatus {
    Pending,
    Approved,
    Claimed,
    Rejected,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppPairingPollResult {
    pub pairing_id: String,
    pub status: AppPairingPollStatus,
    pub expires_at: DateTime<Utc>,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub device_label: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AppAuthStore {
    path: PathBuf,
}

impl AppAuthStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self) -> Result<AppAuthState> {
        if !self.path.exists() {
            return Ok(AppAuthState::default());
        }

        let raw = fs::read_to_string(&self.path)
            .with_context(|| format!("failed to read app auth file {}", self.path.display()))?;
        serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse app auth file {}", self.path.display()))
    }

    pub fn save(&self, state: &AppAuthState) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        let parent = self
            .path
            .parent()
            .context("app auth file path must have a parent directory")?;
        let mut tmp =
            NamedTempFile::new_in(parent).context("failed to create temporary app auth file")?;
        serde_json::to_writer_pretty(&mut tmp, state).context("failed to serialize app auth")?;
        tmp.persist(&self.path)
            .map_err(|err| err.error)
            .with_context(|| {
                format!(
                    "failed to persist app auth state to {}",
                    self.path.display()
                )
            })?;
        Ok(())
    }

    pub fn create_pairing_request(
        &self,
        device_label: String,
        ttl_seconds: u64,
    ) -> Result<AppPairingRequest> {
        let mut state = self.load()?;
        prune_expired_pairings(&mut state);

        let created_at = Utc::now();
        let expires_at = created_at
            .checked_add_signed(Duration::seconds(ttl_seconds as i64))
            .context("pairing ttl overflowed")?;
        let request = AppPairingRequest {
            pairing_id: Uuid::new_v4().to_string(),
            code: generate_pairing_code(),
            device_label,
            created_at,
            expires_at,
            status: AppPairingStatus::Pending,
            approved_device_id: None,
            claimed_at: None,
        };
        state.pending_pairings.push(request.clone());
        self.save(&state)?;
        Ok(request)
    }

    pub fn list_pairings(&self) -> Result<Vec<AppPairingRequest>> {
        let mut state = self.load()?;
        let changed = prune_expired_pairings(&mut state);
        if changed {
            self.save(&state)?;
        }
        Ok(state.pending_pairings)
    }

    pub fn approve_pairing_code(&self, code: &str) -> Result<(AppPairingRequest, AppDeviceRecord)> {
        let mut state = self.load()?;
        prune_expired_pairings(&mut state);

        let pairing = state
            .pending_pairings
            .iter_mut()
            .find(|request| request.code.eq_ignore_ascii_case(code))
            .with_context(|| format!("pairing code not found: {code}"))?;

        if pairing.status != AppPairingStatus::Pending {
            bail!("pairing code is no longer pending: {code}");
        }
        if pairing.expires_at <= Utc::now() {
            bail!("pairing code expired: {code}");
        }

        let device = AppDeviceRecord {
            device_id: Uuid::new_v4().to_string(),
            label: pairing.device_label.clone(),
            created_at: Utc::now(),
            last_seen_at: None,
            revoked_at: None,
            token_hash: None,
        };
        pairing.status = AppPairingStatus::Approved;
        pairing.approved_device_id = Some(device.device_id.clone());
        pairing.claimed_at = None;
        state.devices.push(device.clone());
        let pairing = pairing.clone();
        self.save(&state)?;
        Ok((pairing, device))
    }

    pub fn reject_pairing_code(&self, code: &str) -> Result<AppPairingRequest> {
        let mut state = self.load()?;
        prune_expired_pairings(&mut state);

        let pairing = state
            .pending_pairings
            .iter_mut()
            .find(|request| request.code.eq_ignore_ascii_case(code))
            .with_context(|| format!("pairing code not found: {code}"))?;
        if pairing.status != AppPairingStatus::Pending {
            bail!("pairing code is no longer pending: {code}");
        }
        pairing.status = AppPairingStatus::Rejected;
        let pairing = pairing.clone();
        self.save(&state)?;
        Ok(pairing)
    }

    pub fn poll_pairing(&self, pairing_id: &str) -> Result<AppPairingPollResult> {
        let mut state = self.load()?;
        let changed = prune_expired_pairings(&mut state);
        let pairing_index = state
            .pending_pairings
            .iter()
            .position(|request| request.pairing_id == pairing_id)
            .with_context(|| format!("pairing request not found: {pairing_id}"))?;
        let mut should_save = changed;
        let snapshot = state.pending_pairings[pairing_index].clone();
        let result =
            if snapshot.expires_at <= Utc::now() && snapshot.status == AppPairingStatus::Pending {
                state.pending_pairings[pairing_index].status = AppPairingStatus::Rejected;
                should_save = true;
                AppPairingPollResult {
                    pairing_id: snapshot.pairing_id,
                    status: AppPairingPollStatus::Expired,
                    expires_at: snapshot.expires_at,
                    device_id: None,
                    device_label: Some(snapshot.device_label),
                    token: None,
                }
            } else {
                match snapshot.status {
                    AppPairingStatus::Pending => AppPairingPollResult {
                        pairing_id: snapshot.pairing_id,
                        status: AppPairingPollStatus::Pending,
                        expires_at: snapshot.expires_at,
                        device_id: snapshot.approved_device_id,
                        device_label: Some(snapshot.device_label),
                        token: None,
                    },
                    AppPairingStatus::Rejected => AppPairingPollResult {
                        pairing_id: snapshot.pairing_id,
                        status: if snapshot.expires_at <= Utc::now() {
                            AppPairingPollStatus::Expired
                        } else {
                            AppPairingPollStatus::Rejected
                        },
                        expires_at: snapshot.expires_at,
                        device_id: snapshot.approved_device_id,
                        device_label: Some(snapshot.device_label),
                        token: None,
                    },
                    AppPairingStatus::Approved => {
                        let Some(device_id) = snapshot.approved_device_id.clone() else {
                            bail!(
                                "approved pairing missing device id: {}",
                                snapshot.pairing_id
                            );
                        };

                        if snapshot.claimed_at.is_some() {
                            AppPairingPollResult {
                                pairing_id: snapshot.pairing_id,
                                status: AppPairingPollStatus::Claimed,
                                expires_at: snapshot.expires_at,
                                device_id: Some(device_id),
                                device_label: Some(snapshot.device_label),
                                token: None,
                            }
                        } else {
                            let token = generate_device_token();
                            let token_hash = hash_token(&token);
                            let device = state
                                .devices
                                .iter_mut()
                                .find(|device| device.device_id == device_id)
                                .with_context(|| {
                                    format!(
                                        "pairing {} references missing device {}",
                                        snapshot.pairing_id, device_id
                                    )
                                })?;
                            device.token_hash = Some(token_hash);
                            state.pending_pairings[pairing_index].claimed_at = Some(Utc::now());
                            should_save = true;
                            AppPairingPollResult {
                                pairing_id: snapshot.pairing_id,
                                status: AppPairingPollStatus::Approved,
                                expires_at: snapshot.expires_at,
                                device_id: Some(device_id),
                                device_label: Some(snapshot.device_label),
                                token: Some(token),
                            }
                        }
                    }
                }
            };

        if should_save {
            self.save(&state)?;
        }
        Ok(result)
    }

    pub fn list_devices(&self) -> Result<Vec<AppDeviceRecord>> {
        let state = self.load()?;
        Ok(state.devices)
    }

    pub fn create_device(&self, label: &str) -> Result<(AppDeviceRecord, String)> {
        let trimmed = label.trim();
        if trimmed.is_empty() {
            bail!("device label must not be empty");
        }

        let mut state = self.load()?;
        let token = generate_device_token();
        let device = AppDeviceRecord {
            device_id: Uuid::new_v4().to_string(),
            label: trimmed.to_string(),
            created_at: Utc::now(),
            last_seen_at: None,
            revoked_at: None,
            token_hash: Some(hash_token(&token)),
        };
        state.devices.push(device.clone());
        self.save(&state)?;
        Ok((device, token))
    }

    pub fn rotate_device_token(&self, device_id: &str) -> Result<(AppDeviceRecord, String)> {
        let mut state = self.load()?;
        let device = state
            .devices
            .iter_mut()
            .find(|device| device.device_id == device_id)
            .with_context(|| format!("device not found: {device_id}"))?;
        if device.revoked_at.is_some() {
            bail!("device is revoked and cannot rotate token: {device_id}");
        }

        let token = generate_device_token();
        device.token_hash = Some(hash_token(&token));
        let device = device.clone();
        self.save(&state)?;
        Ok((device, token))
    }

    pub fn revoke_device(&self, device_id: &str) -> Result<AppDeviceRecord> {
        let mut state = self.load()?;
        let device = state
            .devices
            .iter_mut()
            .find(|device| device.device_id == device_id)
            .with_context(|| format!("device not found: {device_id}"))?;
        device.revoked_at = Some(Utc::now());
        device.token_hash = None;
        let device = device.clone();
        self.save(&state)?;
        Ok(device)
    }

    pub fn authenticate_token(&self, token: &str) -> Result<Option<AppDeviceRecord>> {
        let state = self.load()?;
        let token_hash = hash_token(token);
        Ok(state.devices.into_iter().find(|device| {
            device.revoked_at.is_none() && device.token_hash.as_deref() == Some(token_hash.as_str())
        }))
    }

    pub fn touch_last_seen(&self, device_id: &str) -> Result<()> {
        let mut state = self.load()?;
        let device = state
            .devices
            .iter_mut()
            .find(|device| device.device_id == device_id)
            .with_context(|| format!("device not found: {device_id}"))?;
        device.last_seen_at = Some(Utc::now());
        self.save(&state)
    }
}

fn prune_expired_pairings(state: &mut AppAuthState) -> bool {
    let now = Utc::now();
    let mut changed = false;
    for pairing in &mut state.pending_pairings {
        if pairing.status == AppPairingStatus::Pending && pairing.expires_at <= now {
            pairing.status = AppPairingStatus::Rejected;
            changed = true;
        }
    }
    changed
}

fn generate_pairing_code() -> String {
    Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(6)
        .collect::<String>()
        .to_uppercase()
}

fn generate_device_token() -> String {
    format!("mcx_{}", Uuid::new_v4().simple())
}

fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn approved_pairing_returns_token_once() {
        let dir = tempdir().unwrap();
        let store = AppAuthStore::new(dir.path().join("app_auth.json"));

        let pairing = store.create_pairing_request("iPhone".into(), 600).unwrap();
        let (_pairing, device) = store.approve_pairing_code(&pairing.code).unwrap();

        let first_poll = store.poll_pairing(&pairing.pairing_id).unwrap();
        assert_eq!(first_poll.status, AppPairingPollStatus::Approved);
        assert_eq!(
            first_poll.device_id.as_deref(),
            Some(device.device_id.as_str())
        );
        assert!(first_poll.token.is_some());

        let second_poll = store.poll_pairing(&pairing.pairing_id).unwrap();
        assert_eq!(second_poll.status, AppPairingPollStatus::Claimed);
        assert!(second_poll.token.is_none());
    }

    #[test]
    fn authenticate_token_ignores_revoked_device() {
        let dir = tempdir().unwrap();
        let store = AppAuthStore::new(dir.path().join("app_auth.json"));

        let pairing = store.create_pairing_request("MacBook".into(), 600).unwrap();
        let (_pairing, device) = store.approve_pairing_code(&pairing.code).unwrap();
        let poll = store.poll_pairing(&pairing.pairing_id).unwrap();
        let token = poll.token.unwrap();

        assert_eq!(
            store.authenticate_token(&token).unwrap().unwrap().device_id,
            device.device_id
        );

        store.revoke_device(&device.device_id).unwrap();
        assert!(store.authenticate_token(&token).unwrap().is_none());
    }

    #[test]
    fn created_device_returns_token_and_persists_only_hash() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("app_auth.json");
        let store = AppAuthStore::new(path.clone());

        let (device, token) = store.create_device("Linux admin").unwrap();

        assert_eq!(device.label, "Linux admin");
        assert_eq!(
            store.authenticate_token(&token).unwrap().unwrap().device_id,
            device.device_id
        );

        let state = store.load().unwrap();
        let stored = state
            .devices
            .iter()
            .find(|entry| entry.device_id == device.device_id)
            .unwrap();
        assert!(stored.token_hash.is_some());
        assert_ne!(stored.token_hash.as_deref(), Some(token.as_str()));

        let raw = fs::read_to_string(path).unwrap();
        assert!(!raw.contains(token.as_str()));
    }

    #[test]
    fn rotate_device_invalidates_previous_token() {
        let dir = tempdir().unwrap();
        let store = AppAuthStore::new(dir.path().join("app_auth.json"));

        let (device, first_token) = store.create_device("Ops laptop").unwrap();
        let (_device, second_token) = store.rotate_device_token(&device.device_id).unwrap();

        assert_ne!(first_token, second_token);
        assert!(store.authenticate_token(&first_token).unwrap().is_none());
        assert_eq!(
            store
                .authenticate_token(&second_token)
                .unwrap()
                .unwrap()
                .device_id,
            device.device_id
        );
    }

    #[test]
    fn revoked_device_cannot_rotate_token() {
        let dir = tempdir().unwrap();
        let store = AppAuthStore::new(dir.path().join("app_auth.json"));

        let (device, _token) = store.create_device("Revoked Mac").unwrap();
        store.revoke_device(&device.device_id).unwrap();

        let error = store.rotate_device_token(&device.device_id).unwrap_err();
        assert!(error.to_string().contains("cannot rotate token"));
    }
}
