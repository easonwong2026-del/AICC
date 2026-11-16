//! TLS identity for the LAN bridge (PR-B).
//!
//! The bridge serves quota data to e-ink devices over HTTPS.  That needs a
//! certificate the device can pin, and the pin has to survive App upgrades —
//! otherwise every release forces the user to re-pair.
//!
//! The layout this module owns (mode 0700, keys 0600) is read by
//! `bridge_tls.py`:
//!
//! ```text
//! ca.pem        self-signed root, 10 years, its public key is the pin
//! ca.key        root private key
//! server.pem    leaf certificate followed by the root (full chain)
//! server.key    leaf private key
//! identity.json spki / not_after / generation / sans
//! ```
//!
//! Two rules matter more than the rest:
//!
//! * The **root** key is the identity.  Leaves are short lived and get
//!   re-issued whenever they near expiry or their SANs stop matching the
//!   machine, so a laptop that moves between networks keeps working without
//!   touching the pin.  Only a new root changes the pin, which is why it needs
//!   an explicit `reset` and a fresh user confirmation on every device.
//! * Creation is serialised across processes with an advisory lock: two
//!   workers starting at once must not each mint a different root, or the Mac
//!   would present one identity while the settings UI advertised another.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use base64::Engine;
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, SanType, SerialNumber,
};
use sha2::{Digest, Sha256};

/// Leaves are re-issued this long before they expire, so a Mac that is offline
/// for a couple of weeks does not wake up with a certificate the device
/// already considers dead.
const LEAF_LIFETIME_DAYS: i64 = 397;
const LEAF_RENEW_BEFORE_DAYS: i64 = 30;
const ROOT_LIFETIME_DAYS: i64 = 3650;
const ROOT_RENEW_BEFORE_DAYS: i64 = 365;

const CA_CERT: &str = "ca.pem";
const CA_KEY: &str = "ca.key";
const LEAF_CERT: &str = "server.pem";
const LEAF_KEY: &str = "server.key";
const IDENTITY: &str = "identity.json";
/// Symlink pointing at the published generation; swapped atomically.
const CURRENT: &str = "current";
/// Prefix for a generation being built; never published, removed on startup.
const STAGE_PREFIX: &str = ".stage-";
const LOCK: &str = "identity.lock";

#[derive(Debug)]
pub enum CertError {
    Io(io::Error),
    Crypto(String),
    /// The lock is held by another process that is still working.
    Busy,
}

impl std::fmt::Display for CertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CertError::Io(error) => write!(f, "tls identity io: {error}"),
            CertError::Crypto(reason) => write!(f, "tls identity crypto: {reason}"),
            CertError::Busy => write!(f, "tls identity is being written by another process"),
        }
    }
}

impl From<io::Error> for CertError {
    fn from(error: io::Error) -> Self {
        CertError::Io(error)
    }
}

impl From<rcgen::Error> for CertError {
    fn from(error: rcgen::Error) -> Self {
        CertError::Crypto(error.to_string())
    }
}

/// What the bridge and the settings UI need; never contains key material.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TlsIdentity {
    /// Base64 SHA-256 over the root's SubjectPublicKeyInfo — the value a device
    /// pins and a human compares against the Mac.
    pub spki: String,
    pub not_after: String,
    /// Increments only when the root key changes, i.e. when every paired
    /// device must confirm the new fingerprint.
    pub generation: u32,
    pub sans: Vec<String>,
    pub ca_created: bool,
    pub leaf_reissued: bool,
}

pub struct TlsStore {
    /// Root of the certificate directory (`certs/`).
    base: PathBuf,
    /// Injectable so renewal windows can be tested without waiting a year.
    clock: Box<dyn Fn() -> chrono::DateTime<chrono::Utc> + Send + Sync>,
}

impl TlsStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        TlsStore {
            base: directory.into(),
            clock: Box::new(chrono::Utc::now),
        }
    }

    #[cfg(test)]
    pub fn with_clock(
        directory: impl Into<PathBuf>,
        clock: impl Fn() -> chrono::DateTime<chrono::Utc> + Send + Sync + 'static,
    ) -> Self {
        TlsStore {
            base: directory.into(),
            clock: Box::new(clock),
        }
    }

    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        (self.clock)()
    }

    /// Default location shared with the Python bridge.  `EINK_CERT_DIR`
    /// overrides it in both processes, which is how the tests point them at a
    /// throwaway directory without touching the real identity.
    pub fn default_directory() -> PathBuf {
        if let Ok(override_dir) = std::env::var("EINK_CERT_DIR") {
            if !override_dir.trim().is_empty() {
                return PathBuf::from(override_dir);
            }
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join("Library/Application Support/AICC-Dashboard/certs")
    }

    /// Directory the bridge reads: the published generation, or the flat
    /// layout used before generations existed.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn directory(&self) -> PathBuf {
        self.active_dir()
    }

    /// The store root (above the generation symlink).  Only tests need it.
    #[cfg(test)]
    pub fn base_path(&self) -> &Path {
        &self.base
    }

    fn active_dir(&self) -> PathBuf {
        let link = self.base.join(CURRENT);
        if let Ok(target) = fs::read_link(&link) {
            if let Some(name) = target.file_name() {
                let candidate = self.base.join(name);
                if candidate.join(LEAF_CERT).exists() || candidate.join(CA_CERT).exists() {
                    return candidate;
                }
            }
        }
        self.base.clone()
    }

    /// Current identity without touching anything that already exists.
    pub fn read(&self) -> Result<TlsIdentity, CertError> {
        let document = fs::read_to_string(self.active_dir().join(IDENTITY))?;
        let parsed: serde_json::Value = serde_json::from_str(&document)
            .map_err(|error| CertError::Crypto(error.to_string()))?;
        let spki = parsed
            .get("spki")
            .and_then(|value| value.as_str())
            .ok_or_else(|| CertError::Crypto("identity.json has no spki".into()))?;
        Ok(TlsIdentity {
            spki: spki.to_string(),
            not_after: parsed
                .get("not_after")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            generation: parsed
                .get("generation")
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as u32,
            sans: parsed
                .get("sans")
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|value| value.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            ca_created: false,
            leaf_reissued: false,
        })
    }

    /// Create, renew or reuse the identity.  Safe to call on every launch.
    pub fn ensure(&self, extra_sans: &[String]) -> Result<TlsIdentity, CertError> {
        self.prepare()?;
        let _guard = self.lock()?;
        self.ensure_locked(extra_sans, false)
    }

    /// Throw the root away.  Every paired device has to confirm the new
    /// fingerprint: this is the "I think someone copied my key" button.
    pub fn reset(&self) -> Result<TlsIdentity, CertError> {
        self.prepare()?;
        let _guard = self.lock()?;
        self.ensure_locked(&[], true)
    }

    fn prepare(&self) -> Result<(), CertError> {
        fs::create_dir_all(&self.base)?;
        fs::set_permissions(&self.base, fs::Permissions::from_mode(0o700))?;
        // Half-written generations from a crash are never trusted: they are
        // removed here so they cannot accumulate or be mistaken for state.
        if let Ok(entries) = fs::read_dir(&self.base) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name.to_string_lossy().starts_with(STAGE_PREFIX) {
                    let _ = fs::remove_dir_all(entry.path());
                }
            }
        }
        Ok(())
    }

    fn lock(&self) -> Result<File, CertError> {
        let path = self.base.join(LOCK);
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .mode(0o600)
            .open(path)?;
        for _ in 0..40 {
            if file.try_lock().is_ok() {
                return Ok(file);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        Err(CertError::Busy)
    }

    fn ensure_locked(
        &self,
        extra_sans: &[String],
        force_new_root: bool,
    ) -> Result<TlsIdentity, CertError> {
        let active = self.active_dir();
        let wanted = san_set(extra_sans);
        let previous = self.read().map(|identity| identity.generation).unwrap_or(0);

        let existing_root = if force_new_root {
            None
        } else {
            self.load_root(&active)?
        };
        let root_created = existing_root.is_none();
        let (root_key, root_pem) = match existing_root {
            Some(root) => (root.key, root.pem),
            None => self.mint_root()?,
        };

        if !root_created && !force_new_root {
            if let Ok(state) = self.leaf_state_in(&active) {
                let key_ok = self
                    .leaf_key_in(&active)
                    .map(|key| key_matches_certificate(&key, &state))
                    .unwrap_or(false);
                let fresh =
                    state.not_after - self.now() >= chrono::Duration::days(LEAF_RENEW_BEFORE_DAYS);
                if key_ok && fresh && covers(&state.sans, &wanted.names) {
                    // Nothing to issue: only the published metadata is rewritten,
                    // and it is the certificate's own notAfter.
                    let identity = TlsIdentity {
                        spki: pin(&root_key),
                        not_after: state.not_after.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                        generation: previous.max(1),
                        sans: state.sans.clone(),
                        ca_created: false,
                        leaf_reissued: false,
                    };
                    self.write_identity_in(&active, &identity)?;
                    return Ok(identity);
                }
            }
        }

        // Build a complete generation first; nothing is published until every
        // file in it has been verified.
        let stage = self
            .base
            .join(format!("{STAGE_PREFIX}{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&stage)?;
        fs::set_permissions(&stage, fs::Permissions::from_mode(0o700))?;
        let outcome = self.build_generation(
            &stage,
            &root_key,
            &root_pem,
            &wanted,
            previous,
            root_created,
        );
        let identity = match outcome {
            Ok(identity) => identity,
            Err(error) => {
                let _ = fs::remove_dir_all(&stage);
                return Err(error);
            }
        };
        self.publish(&stage)?;
        Ok(identity)
    }

    fn build_generation(
        &self,
        stage: &Path,
        root_key: &KeyPair,
        root_pem: &str,
        wanted: &SanSet,
        previous_generation: u32,
        root_created: bool,
    ) -> Result<TlsIdentity, CertError> {
        write_atomic(&stage.join(CA_CERT), root_pem.as_bytes())?;
        self.write_private(&stage.join(CA_KEY), root_key.serialize_pem().as_bytes())?;

        // Reuse the leaf key when one exists, so a renewal does not disturb
        // anything pinned to the leaf (nothing should be — the pin is the root —
        // but rotating keys unnecessarily is still worse).
        let leaf_key = self
            .leaf_key_in(&self.active_dir())
            .unwrap_or(KeyPair::generate()?);
        self.write_leaf_into(stage, &leaf_key, root_key, root_pem, wanted)?;

        let state = self.leaf_state_in(stage)?;
        self.verify_generation(stage, root_key, wanted, &state)?;

        let generation = if root_created {
            previous_generation + 1
        } else {
            previous_generation.max(1)
        };
        let identity = TlsIdentity {
            spki: pin(root_key),
            not_after: state.not_after.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            generation,
            sans: state.sans,
            ca_created: root_created,
            leaf_reissued: true,
        };
        self.write_identity_in(stage, &identity)?;
        Ok(identity)
    }

    /// Everything inside a generation must be consistent before it is published.
    fn verify_generation(
        &self,
        directory: &Path,
        root_key: &KeyPair,
        wanted: &SanSet,
        leaf: &LeafState,
    ) -> Result<(), CertError> {
        let root = self.certificate_state_in(directory, CA_CERT)?;
        let leaf_key = self
            .leaf_key_in(directory)
            .ok_or_else(|| CertError::Crypto("generation has no readable leaf key".into()))?;
        let root_key_on_disk = fs::read_to_string(directory.join(CA_KEY))
            .ok()
            .and_then(|pem| KeyPair::from_pem(&pem).ok())
            .ok_or_else(|| CertError::Crypto("generation has no readable root key".into()))?;

        if !key_matches_certificate(&leaf_key, leaf) {
            return Err(CertError::Crypto(
                "leaf key does not match the leaf certificate".into(),
            ));
        }
        if !key_matches_certificate(&root_key_on_disk, &root) {
            return Err(CertError::Crypto(
                "root key does not match the root certificate".into(),
            ));
        }
        if pin(&root_key_on_disk) != pin(root_key) {
            return Err(CertError::Crypto(
                "root key on disk is not the key used to sign".into(),
            ));
        }
        if !covers(&leaf.sans, &wanted.names) {
            return Err(CertError::Crypto(
                "generation does not cover every wanted address".into(),
            ));
        }
        if leaf.not_after <= self.now() {
            return Err(CertError::Crypto(
                "generation leaf is already expired".into(),
            ));
        }
        if leaf.not_after > root.not_after {
            return Err(CertError::Crypto("leaf outlives its root".into()));
        }
        // Cryptographic chain check, not just a name comparison.
        verify_chain(directory, LEAF_CERT, CA_CERT)?;
        Ok(())
    }

    fn publish(&self, stage: &Path) -> Result<(), CertError> {
        let name = format!("gen-{}", uuid::Uuid::new_v4().simple());
        let target = self.base.join(&name);
        fs::rename(stage, &target)?;
        sync_directory(&self.base);

        // Atomic switch: the symlink is replaced, never edited in place, so a
        // reader sees either the old generation or the new one, never a mix.
        let temporary = self.base.join(format!("{CURRENT}.tmp"));
        let _ = fs::remove_file(&temporary);
        std::os::unix::fs::symlink(&name, &temporary)?;
        fs::rename(&temporary, self.base.join(CURRENT))?;
        sync_directory(&self.base);
        self.cleanup(&name);
        Ok(())
    }

    /// Keep the live generation and one predecessor for rollback; drop the rest.
    fn cleanup(&self, live: &str) {
        let mut generations: Vec<String> = match fs::read_dir(&self.base) {
            Ok(entries) => entries
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .map(|entry| entry.file_name().to_string_lossy().to_string())
                .filter(|name| name.starts_with("gen-"))
                .collect(),
            Err(_) => return,
        };
        generations.sort();
        generations.retain(|name| name != live);
        while generations.len() > 1 {
            let stale = generations.remove(0);
            let _ = fs::remove_dir_all(self.base.join(stale));
        }
    }

    fn load_root(&self, directory: &Path) -> Result<Option<RootMaterial>, CertError> {
        let (cert_path, key_path) = (directory.join(CA_CERT), directory.join(CA_KEY));
        if !cert_path.exists() || !key_path.exists() {
            // A root without its key cannot sign leaves; and a root we cannot
            // read is not a root we can keep.  Either way a new one is minted,
            // which changes the pin and forces devices to re-confirm.
            return Ok(None);
        }
        let pem = fs::read_to_string(&cert_path)?;
        let key_pem = fs::read_to_string(&key_path)?;
        let key = match KeyPair::from_pem(&key_pem) {
            Ok(key) => key,
            Err(_) => return Ok(None),
        };
        let state = match self.certificate_state_in(directory, CA_CERT) {
            Ok(state) => state,
            Err(_) => return Ok(None),
        };
        if !key_matches_certificate(&key, &state) {
            return Ok(None);
        }
        let pem = if state.not_after - self.now() < chrono::Duration::days(ROOT_RENEW_BEFORE_DAYS) {
            // Same key, new dates: the pin stays, the devices stay paired.
            self.sign_root(&key)?
        } else {
            pem
        };
        Ok(Some(RootMaterial { key, pem }))
    }

    fn mint_root(&self) -> Result<(KeyPair, String), CertError> {
        let key = KeyPair::generate()?;
        let pem = self.sign_root(&key)?;
        Ok((key, pem))
    }

    /// Sign (or re-sign) the root certificate with `key`.
    ///
    /// Re-signing on the same key is how a root near expiry is renewed without
    /// touching the pin: the public key — and therefore the SPKI a device
    /// pinned — does not change, so no one has to re-pair.
    fn sign_root(&self, key: &KeyPair) -> Result<String, CertError> {
        let mut params = root_params(self.now())?;
        params.serial_number = Some(serial());
        Ok(params.self_signed(key)?.pem())
    }

    fn write_leaf_into(
        &self,
        directory: &Path,
        leaf_key: &KeyPair,
        root_key: &KeyPair,
        root_pem: &str,
        wanted: &SanSet,
    ) -> Result<(), CertError> {
        let certificate = leaf_certificate(self.now(), leaf_key, root_key, root_pem, wanted)?;
        let chain = format!("{}{}", certificate.pem(), root_pem);
        write_atomic(&directory.join(LEAF_CERT), chain.as_bytes())?;
        self.write_private(
            &directory.join(LEAF_KEY),
            leaf_key.serialize_pem().as_bytes(),
        )?;
        Ok(())
    }

    fn leaf_key_in(&self, directory: &Path) -> Option<KeyPair> {
        fs::read_to_string(directory.join(LEAF_KEY))
            .ok()
            .and_then(|pem| KeyPair::from_pem(&pem).ok())
    }

    /// Expiry, SANs and SPKI as they really are in `server.pem`.
    fn leaf_state_in(&self, directory: &Path) -> Result<LeafState, CertError> {
        self.certificate_state_in(directory, LEAF_CERT)
    }

    fn certificate_state_in(&self, directory: &Path, name: &str) -> Result<LeafState, CertError> {
        let der = read_certificate_der(&directory.join(name))?;
        let (_, certificate) = x509_parser::parse_x509_certificate(&der)
            .map_err(|error| CertError::Crypto(format!("{name} is not X.509: {error}")))?;
        let validity = certificate.validity();
        let not_after = chrono::DateTime::from_timestamp(validity.not_after.timestamp(), 0)
            .ok_or_else(|| CertError::Crypto(format!("{name} notAfter is out of range")))?;
        let mut sans: Vec<String> = Vec::new();
        if let Ok(Some(extension)) = certificate.subject_alternative_name() {
            for name in &extension.value.general_names {
                match name {
                    x509_parser::extensions::GeneralName::DNSName(value) => {
                        sans.push(value.to_string())
                    }
                    x509_parser::extensions::GeneralName::IPAddress(bytes) if bytes.len() == 4 => {
                        sans.push(
                            std::net::Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3])
                                .to_string(),
                        )
                    }
                    x509_parser::extensions::GeneralName::IPAddress(bytes) if bytes.len() == 16 => {
                        let mut octets = [0u8; 16];
                        octets.copy_from_slice(bytes);
                        sans.push(std::net::Ipv6Addr::from(octets).to_string());
                    }
                    _ => {}
                }
            }
        }
        sans.sort();
        sans.dedup();
        Ok(LeafState {
            not_after,
            sans,
            spki: certificate.public_key().raw.to_vec(),
        })
    }

    fn write_identity_in(&self, directory: &Path, identity: &TlsIdentity) -> Result<(), CertError> {
        let document = serde_json::json!({
            "version": 1,
            "spki": identity.spki,
            "not_after": identity.not_after,
            "generation": identity.generation,
            "sans": identity.sans,
        });
        write_atomic(&directory.join(IDENTITY), document.to_string().as_bytes())
    }

    fn write_private(&self, path: &Path, contents: &[u8]) -> Result<(), CertError> {
        let temporary = path.with_extension("key.tmp");
        {
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(contents)?;
            file.sync_all()?;
        }
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
        fs::rename(&temporary, path)?;
        Ok(())
    }
}

struct RootMaterial {
    key: KeyPair,
    pem: String,
}

struct LeafState {
    not_after: chrono::DateTime<chrono::Utc>,
    sans: Vec<String>,
    /// DER SubjectPublicKeyInfo from the certificate itself.
    spki: Vec<u8>,
}

#[derive(Debug, Default)]
struct SanSet {
    names: Vec<String>,
}

fn san_set(extra: &[String]) -> SanSet {
    let mut names: Vec<String> = vec!["127.0.0.1".into(), "::1".into(), "localhost".into()];
    if let Ok(hostname) = hostname() {
        names.push(hostname.clone());
        names.push(format!("{hostname}.local"));
    }
    if let Some(address) = primary_address() {
        names.push(address.to_string());
    }
    for name in extra {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            names.push(trimmed.to_string());
        }
    }
    names.sort();
    names.dedup();
    SanSet { names }
}

/// A certificate only has to cover what the device connects to; anything less
/// and the handshake fails with a hostname error that looks like a bug.
fn covers(existing: &[String], wanted: &[String]) -> bool {
    wanted
        .iter()
        .all(|name| existing.iter().any(|have| have == name))
}

fn hostname() -> Result<String, std::io::Error> {
    // No dependency on libc: the host name is already in the environment or
    // resolvable through the loopback alias.
    if let Ok(name) = std::env::var("HOSTNAME") {
        if !name.is_empty() {
            return Ok(name);
        }
    }
    let output = std::process::Command::new("hostname").arg("-s").output()?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The address a device would reach, without sending a packet: connecting a
/// UDP socket only asks the routing table which source it would use.
fn primary_address() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect(SocketAddr::from(([192, 0, 2, 1], 9))).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(address) if !address.is_loopback() && !address.is_link_local() => Some(address),
        _ => None,
    }
}

fn serial() -> SerialNumber {
    let bytes = uuid::Uuid::new_v4().as_bytes().to_vec();
    SerialNumber::from(bytes)
}

fn stamp(moment: chrono::DateTime<chrono::Utc>) -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp(moment.timestamp()).expect("timestamp in range")
}

/// Base64 SHA-256 over the DER SubjectPublicKeyInfo of the root key.
fn pin(key: &KeyPair) -> String {
    use rcgen::PublicKeyData;
    let digest = Sha256::digest(key.subject_public_key_info());
    base64::engine::general_purpose::STANDARD.encode(digest)
}

fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), CertError> {
    let temporary = path.with_extension("tmp");
    {
        let mut file = File::create(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
    }
    fs::rename(&temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    fn store() -> (tempfile::TempDir, TlsStore) {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = TlsStore::new(directory.path().join("certs"));
        (directory, store)
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).expect("metadata").mode() & 0o777
    }

    #[test]
    fn first_run_mints_a_root_and_a_leaf() {
        let (_keep, store) = store();
        let identity = store.ensure(&[]).expect("ensure");
        assert!(
            identity.ca_created,
            "a fresh directory has no root to reuse"
        );
        assert_eq!(identity.generation, 1);
        assert_eq!(
            identity.spki.len(),
            44,
            "base64 of a SHA-256 is 44 characters"
        );
        for name in [CA_CERT, CA_KEY, LEAF_CERT, LEAF_KEY, IDENTITY] {
            assert!(store.directory().join(name).exists(), "{name} missing");
        }
        assert_eq!(
            mode(&store.directory()),
            0o700,
            "certificate directory must be private"
        );
        assert_eq!(mode(&store.directory().join(CA_KEY)), 0o600);
        assert_eq!(mode(&store.directory().join(LEAF_KEY)), 0o600);
    }

    #[test]
    fn relaunch_reuses_the_same_identity() {
        let (_keep, store) = store();
        let first = store.ensure(&[]).expect("ensure");
        let second = store.ensure(&[]).expect("ensure again");
        assert!(!second.ca_created, "the root must survive a relaunch");
        assert!(
            !second.leaf_reissued,
            "a valid leaf must not be re-issued on every launch"
        );
        assert_eq!(first.spki, second.spki);
        assert_eq!(first.generation, second.generation);
    }

    #[test]
    fn the_leaf_chains_to_the_pinned_root() {
        let (_keep, store) = store();
        let identity = store.ensure(&[]).expect("ensure");
        let chain = fs::read_to_string(store.directory().join(LEAF_CERT)).expect("chain");
        let leaf = chain
            .split("-----END CERTIFICATE-----")
            .next()
            .expect("leaf")
            .to_owned()
            + "-----END CERTIFICATE-----\n";
        let leaf_path = store.directory().join("leaf-only.pem");
        fs::write(&leaf_path, leaf).expect("leaf");
        // LibreSSL ships with macOS; this is exactly the check the bridge's
        // Python side relies on, so test it against a real verifier.
        let verdict = std::process::Command::new("/usr/bin/openssl")
            .arg("verify")
            .arg("-CAfile")
            .arg(store.directory().join(CA_CERT))
            .arg(&leaf_path)
            .output()
            .expect("openssl verify");
        assert!(
            verdict.status.success(),
            "leaf does not chain to the pinned root: {}",
            String::from_utf8_lossy(&verdict.stderr)
        );
        let pinned = pin(&KeyPair::from_pem(
            &fs::read_to_string(store.directory().join(CA_KEY)).unwrap(),
        )
        .unwrap());
        assert_eq!(
            pinned, identity.spki,
            "the published pin must be the root's SPKI"
        );
    }

    fn clock_window(
        expiry: chrono::DateTime<chrono::Utc>,
        days_before: i64,
    ) -> impl Fn() -> chrono::DateTime<chrono::Utc> + Send + Sync + 'static {
        let moment = expiry - chrono::Duration::days(days_before);
        move || moment
    }

    fn identity_not_after(store: &TlsStore) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(&store.read().expect("identity").not_after)
            .expect("parse")
            .with_timezone(&chrono::Utc)
    }

    fn certificate_not_after(store: &TlsStore, name: &str) -> chrono::DateTime<chrono::Utc> {
        store
            .certificate_state_in(&store.directory(), name)
            .expect("certificate")
            .not_after
    }

    #[test]
    fn metadata_never_drifts_ahead_of_the_certificate() {
        let (_keep, store) = store();
        let first = store.ensure(&[]).expect("ensure");
        // A daily launch must not push the published expiry forward: the leaf
        // is not re-issued, so its notAfter cannot change.
        for _ in 0..5 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            let again = store.ensure(&[]).expect("ensure again");
            assert!(!again.leaf_reissued);
            assert_eq!(again.not_after, first.not_after, "identity expiry drifted");
        }
        let on_pem = certificate_not_after(&store, LEAF_CERT);
        assert_eq!(
            identity_not_after(&store),
            on_pem,
            "identity.json disagrees with server.pem"
        );
    }

    #[test]
    fn thirty_one_days_before_expiry_the_leaf_is_left_alone() {
        let (_keep, store) = store();
        let first = store.ensure(&[]).expect("ensure");
        let expiry = certificate_not_after(&store, LEAF_CERT);
        let later = TlsStore::with_clock(store.base_path().to_path_buf(), clock_window(expiry, 31));
        let second = later.ensure(&[]).expect("ensure");
        assert!(
            !second.leaf_reissued,
            "31 days is outside the 30 day renewal window"
        );
        assert_eq!(second.not_after, first.not_after);
        assert_eq!(
            certificate_not_after(&store, LEAF_CERT),
            expiry,
            "the certificate itself must not change"
        );
    }

    #[test]
    fn twenty_nine_days_before_expiry_the_leaf_is_renewed_under_the_same_pin() {
        let (_keep, store) = store();
        let first = store.ensure(&[]).expect("ensure");
        let expiry = certificate_not_after(&store, LEAF_CERT);
        let later = TlsStore::with_clock(store.base_path().to_path_buf(), clock_window(expiry, 29));
        let second = later.ensure(&[]).expect("ensure");
        assert!(second.leaf_reissued, "29 days is inside the renewal window");
        assert_eq!(second.spki, first.spki, "renewal must not change the pin");
        assert!(
            certificate_not_after(&store, LEAF_CERT) > expiry,
            "the new leaf must outlive the old one"
        );
        assert_eq!(
            identity_not_after(&store),
            certificate_not_after(&store, LEAF_CERT),
            "identity.json must publish the new certificate's real expiry"
        );
    }

    #[test]
    fn an_expired_leaf_is_replaced_not_reported_as_valid() {
        let (_keep, store) = store();
        store.ensure(&[]).expect("ensure");
        let expiry = certificate_not_after(&store, LEAF_CERT);
        let after_expiry =
            TlsStore::with_clock(store.base_path().to_path_buf(), clock_window(expiry, -10));
        let renewed = after_expiry.ensure(&[]).expect("ensure");
        assert!(
            renewed.leaf_reissued,
            "an expired certificate must be replaced"
        );
        let published = identity_not_after(&store);
        assert!(
            published > after_expiry.now(),
            "a new leaf must be valid now"
        );
        assert!(published > expiry);
    }

    #[test]
    fn an_unreadable_certificate_is_replaced_instead_of_trusted() {
        let (_keep, store) = store();
        let first = store.ensure(&[]).expect("ensure");
        fs::write(
            store.directory().join(LEAF_CERT),
            b"-----BEGIN CERTIFICATE-----\nnot base64\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        let recovered = store.ensure(&[]).expect("ensure");
        assert!(recovered.leaf_reissued);
        assert_eq!(
            recovered.spki, first.spki,
            "the root, and therefore the pin, is untouched"
        );
        assert_eq!(
            identity_not_after(&store),
            certificate_not_after(&store, LEAF_CERT)
        );
    }

    #[test]
    fn a_root_near_expiry_is_renewed_on_the_same_key() {
        let (_keep, store) = store();
        let first = store.ensure(&[]).expect("ensure");
        let root_expiry = certificate_not_after(&store, CA_CERT);
        let later = TlsStore::with_clock(
            store.base_path().to_path_buf(),
            clock_window(root_expiry, 100),
        );
        let renewed = later.ensure(&[]).expect("ensure");
        assert_eq!(
            renewed.spki, first.spki,
            "renewing the root must not change the pin"
        );
        assert_eq!(
            renewed.generation, first.generation,
            "and must not force a re-pair"
        );
        assert!(certificate_not_after(&store, CA_CERT) > root_expiry);
    }

    #[test]
    fn concurrent_resets_still_move_generation_forward() {
        let (_keep, store) = store();
        let first = store.ensure(&[]).expect("ensure");
        let mut handles = Vec::new();
        for _ in 0..4 {
            let store = TlsStore::new(store.base_path().to_path_buf());
            handles.push(std::thread::spawn(move || {
                store.reset().expect("reset").generation
            }));
        }
        let generations: Vec<u32> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert!(
            generations.iter().all(|value| *value > first.generation),
            "a reset must always publish a newer generation: {generations:?}"
        );
        let final_generation = store.read().expect("identity").generation;
        assert!(
            final_generation >= first.generation + 4,
            "four resets must produce four new generations, got {final_generation}"
        );
    }

    #[test]
    fn generation_only_moves_forward() {
        let (_keep, store) = store();
        let first = store.ensure(&[]).expect("ensure");
        let second = store.reset().expect("reset");
        let third = store.reset().expect("reset");
        assert!(second.generation > first.generation);
        assert!(third.generation > second.generation);
        assert_ne!(first.spki, third.spki);
    }

    #[test]
    fn a_new_address_re_issues_the_leaf_without_touching_the_pin() {
        let (_keep, store) = store();
        let first = store.ensure(&[]).expect("ensure");
        let second = store.ensure(&["192.0.2.55".to_string()]).expect("ensure");
        assert!(
            second.leaf_reissued,
            "a SAN the leaf does not cover must re-issue it"
        );
        assert!(second.sans.iter().any(|name| name == "192.0.2.55"));
        assert_eq!(second.spki, first.spki);
        let third = store.ensure(&["192.0.2.55".to_string()]).expect("ensure");
        assert!(
            !third.leaf_reissued,
            "the same address must not re-issue twice"
        );
    }

    #[test]
    fn reset_rotates_the_root_and_forces_a_new_confirmation() {
        let (_keep, store) = store();
        let first = store.ensure(&[]).expect("ensure");
        let rotated = store.reset().expect("reset");
        assert!(rotated.ca_created);
        assert_ne!(
            rotated.spki, first.spki,
            "reset must produce a different root"
        );
        assert!(
            rotated.generation > first.generation,
            "devices compare the generation"
        );
    }

    fn generation_dirs(store: &TlsStore) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(store.base_path())
            .expect("base")
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with("gen-"))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_half_written_generation_never_becomes_current() {
        let (_keep, store) = store();
        store.ensure(&[]).expect("ensure");
        // Simulate a crash in the middle of issuing: the certificate landed in
        // a staging directory but the key never did, and nothing was published.
        let stage = store.base_path().join(format!("{STAGE_PREFIX}crash"));
        fs::create_dir_all(&stage).unwrap();
        fs::copy(store.directory().join(LEAF_CERT), stage.join(LEAF_CERT)).unwrap();
        let before = store.read().expect("identity").spki;

        let recovered = store.ensure(&[]).expect("ensure after a crash");
        assert!(
            !stage.exists(),
            "a staging directory must be cleaned up, never trusted"
        );
        assert_eq!(recovered.spki, before, "the published root must not move");
        let active = store.directory();
        assert!(
            active.join(LEAF_KEY).exists(),
            "a usable identity needs its key"
        );
        let key = store.leaf_key_in(&active).expect("leaf key");
        let state = store.leaf_state_in(&active).expect("leaf state");
        assert!(
            key_matches_certificate(&key, &state),
            "certificate and key must pair"
        );
    }

    #[test]
    fn a_published_generation_that_lost_its_key_is_repaired() {
        let (_keep, store) = store();
        let first = store.ensure(&[]).expect("ensure");
        // The exact failure mode this refactor exists for: the certificate is
        // valid and covers every address, but the key is gone.
        fs::remove_file(store.directory().join(LEAF_KEY)).expect("remove key");

        let repaired = store.ensure(&[]).expect("ensure after losing the key");
        assert!(
            repaired.leaf_reissued,
            "a certificate without its key is not usable"
        );
        assert_eq!(
            repaired.spki, first.spki,
            "the root and the pin are untouched"
        );
        let active = store.directory();
        assert!(active.join(LEAF_KEY).exists());
        let key = store.leaf_key_in(&active).expect("leaf key");
        let state = store.leaf_state_in(&active).expect("leaf state");
        assert!(key_matches_certificate(&key, &state));
        assert_eq!(
            identity_not_after(&store),
            certificate_not_after(&store, LEAF_CERT)
        );
    }

    #[test]
    fn a_key_that_belongs_to_another_certificate_is_replaced() {
        let (_keep, store) = store();
        store.ensure(&[]).expect("ensure");
        let stranger = KeyPair::generate().expect("keypair");
        fs::write(store.directory().join(LEAF_KEY), stranger.serialize_pem()).unwrap();

        let repaired = store.ensure(&[]).expect("ensure after a key swap");
        assert!(repaired.leaf_reissued);
        let active = store.directory();
        let key = store.leaf_key_in(&active).expect("leaf key");
        let state = store.leaf_state_in(&active).expect("leaf state");
        assert!(
            key_matches_certificate(&key, &state),
            "the mismatched key must not survive"
        );
    }

    #[test]
    fn the_previous_generation_is_kept_for_rollback() {
        let (_keep, store) = store();
        store.ensure(&[]).expect("ensure");
        let first_generation = store.directory();
        store.reset().expect("reset");
        let generations = generation_dirs(&store);
        assert!(
            generations.len() >= 2,
            "keep the old generation until the new one proves itself"
        );
        assert!(
            first_generation.exists(),
            "the replaced generation is still on disk"
        );
        assert!(store.directory().exists());
    }

    #[test]
    fn the_identity_file_never_contains_key_material() {
        let (_keep, store) = store();
        store.ensure(&[]).expect("ensure");
        let document = fs::read_to_string(store.directory().join(IDENTITY)).expect("identity");
        assert!(!document.contains("PRIVATE"));
        assert!(!document.to_lowercase().contains("private_key"));
    }

    #[test]
    fn concurrent_callers_agree_on_one_identity() {
        let (_keep, store) = store();
        let mut handles = Vec::new();
        for _ in 0..4 {
            let store = TlsStore::new(store.base_path().to_path_buf());
            handles.push(std::thread::spawn(move || {
                store.ensure(&[]).expect("ensure").spki
            }));
        }
        let pins: Vec<String> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert!(
            pins.windows(2).all(|pair| pair[0] == pair[1]),
            "two workers minted different roots: {pins:?}"
        );
    }
}

/// Read the first certificate of a PEM file as DER.
fn read_certificate_der(path: &Path) -> Result<Vec<u8>, CertError> {
    let text = fs::read_to_string(path)?;
    let body = text
        .split("-----BEGIN CERTIFICATE-----")
        .nth(1)
        .ok_or_else(|| CertError::Crypto(format!("{} holds no certificate", path.display())))?
        .split("-----END CERTIFICATE-----")
        .next()
        .ok_or_else(|| CertError::Crypto(format!("{} is truncated", path.display())))?;
    base64::engine::general_purpose::STANDARD
        .decode(body.trim().replace(['\n', '\r'], ""))
        .map_err(|error| CertError::Crypto(format!("{} is not base64: {error}", path.display())))
}

/// A leaf is only usable if it is signed by the certificate above it.
fn verify_chain(directory: &Path, leaf: &str, issuer: &str) -> Result<(), CertError> {
    let leaf_der = read_certificate_der(&directory.join(leaf))?;
    let issuer_der = read_certificate_der(&directory.join(issuer))?;
    let (_, leaf) = x509_parser::parse_x509_certificate(&leaf_der)
        .map_err(|error| CertError::Crypto(format!("{leaf} is not X.509: {error}")))?;
    let (_, issuer_certificate) = x509_parser::parse_x509_certificate(&issuer_der)
        .map_err(|error| CertError::Crypto(format!("{issuer} is not X.509: {error}")))?;
    leaf.verify_signature(Some(issuer_certificate.public_key()))
        .map_err(|error| CertError::Crypto(format!("leaf is not signed by the root: {error}")))?;
    Ok(())
}

/// Does this private key belong to the certificate in `state`?
fn key_matches_certificate(key: &KeyPair, state: &LeafState) -> bool {
    use rcgen::PublicKeyData;
    key.subject_public_key_info() == state.spki
}

fn root_params(now: chrono::DateTime<chrono::Utc>) -> Result<CertificateParams, CertError> {
    let mut params = CertificateParams::new(Vec::new())?;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    params
        .distinguished_name
        .push(DnType::CommonName, "AICC Bridge Root");
    params
        .distinguished_name
        .push(DnType::OrganizationName, "AICC");
    params.not_before = stamp(now - chrono::Duration::days(1));
    params.not_after = stamp(now + chrono::Duration::days(ROOT_LIFETIME_DAYS));
    Ok(params)
}

fn leaf_certificate(
    now: chrono::DateTime<chrono::Utc>,
    leaf_key: &KeyPair,
    root_key: &KeyPair,
    root_pem: &str,
    wanted: &SanSet,
) -> Result<rcgen::Certificate, CertError> {
    let mut params = CertificateParams::new(Vec::new())?;
    params.is_ca = IsCa::ExplicitNoCa;
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params
        .distinguished_name
        .push(DnType::CommonName, "AICC Bridge");
    params
        .distinguished_name
        .push(DnType::OrganizationName, "AICC");
    params.subject_alt_names = wanted
        .names
        .iter()
        .map(|name| match name.parse::<IpAddr>() {
            Ok(address) => SanType::IpAddress(address),
            Err(_) => SanType::DnsName(name.clone().try_into().expect("san")),
        })
        .collect();
    params.not_before = stamp(now - chrono::Duration::days(1));
    params.not_after = stamp(now + chrono::Duration::days(LEAF_LIFETIME_DAYS));
    params.serial_number = Some(serial());
    // The issuer's subject must come from the root certificate on disk,
    // otherwise the authority key identifier stops matching and a device
    // cannot build the chain.
    let issuer = Issuer::from_ca_cert_pem(root_pem, root_key)?;
    Ok(params.signed_by(leaf_key, &issuer)?)
}

fn sync_directory(path: &Path) {
    if let Ok(handle) = File::open(path) {
        let _ = handle.sync_all();
    }
}
