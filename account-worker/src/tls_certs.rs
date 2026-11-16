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
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer,
    KeyPair, KeyUsagePurpose, SanType, SerialNumber,
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
    directory: PathBuf,
}

impl TlsStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        TlsStore { directory: directory.into() }
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

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Current identity without touching anything that already exists.
    pub fn read(&self) -> Result<TlsIdentity, CertError> {
        let document = fs::read_to_string(self.directory.join(IDENTITY))?;
        let parsed: serde_json::Value =
            serde_json::from_str(&document).map_err(|error| CertError::Crypto(error.to_string()))?;
        let spki = parsed
            .get("spki")
            .and_then(|value| value.as_str())
            .ok_or_else(|| CertError::Crypto("identity.json has no spki".into()))?;
        Ok(TlsIdentity {
            spki: spki.to_string(),
            not_after: parsed.get("not_after").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            generation: parsed.get("generation").and_then(|v| v.as_u64()).unwrap_or(1) as u32,
            sans: parsed
                .get("sans")
                .and_then(|v| v.as_array())
                .map(|items| items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
            ca_created: false,
            leaf_reissued: false,
        })
    }

    /// Create, renew or reuse the identity.  Safe to call on every launch.
    ///
    /// `extra_sans` lets the host add addresses it actually bound (an extra
    /// interface, a fixed alias) on top of the ones discovered here.
    pub fn ensure(&self, extra_sans: &[String]) -> Result<TlsIdentity, CertError> {
        fs::create_dir_all(&self.directory)?;
        fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700))?;
        let guard = self.lock()?;
        let outcome = self.ensure_locked(extra_sans);
        drop(guard);
        outcome
    }

    /// Throw the root away.  Every paired device has to confirm the new
    /// fingerprint: this is the "I think someone copied my key" button.
    pub fn reset(&self) -> Result<TlsIdentity, CertError> {
        fs::create_dir_all(&self.directory)?;
        fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700))?;
        let previous = self.read().map(|identity| identity.generation).unwrap_or(0);
        let guard = self.lock()?;
        for name in [CA_CERT, CA_KEY, LEAF_CERT, LEAF_KEY, IDENTITY] {
            let path = self.directory.join(name);
            if path.exists() {
                fs::remove_file(&path)?;
            }
        }
        let mut identity = self.ensure_locked(&[])?;
        // The generation only ever moves forward: a device that pinned
        // generation 3 must not be told the rotated root is still 3.
        if identity.generation <= previous {
            identity.generation = previous + 1;
            self.write_identity(&identity)?;
        }
        drop(guard);
        Ok(identity)
    }

    fn lock(&self) -> Result<File, CertError> {
        let path = self.directory.join(LOCK);
        let file = OpenOptions::new().create(true).write(true).mode(0o600).open(path)?;
        for _ in 0..40 {
            if file.try_lock().is_ok() {
                return Ok(file);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        Err(CertError::Busy)
    }

    fn ensure_locked(&self, extra_sans: &[String]) -> Result<TlsIdentity, CertError> {
        let wanted = san_set(extra_sans);
        let mut ca_created = false;

        let (root_key, root_pem, mut generation) = match self.load_root()? {
            Some(existing) => existing,
            None => {
                let (key, pem) = self.mint_root()?;
                ca_created = true;
                (key, pem, 1)
            }
        };

        let (leaf_key, leaf_reissued, leaf_not_after) = self.load_or_mint_leaf(&root_key, &root_pem, &wanted)?;
        let spki = pin(&root_key);

        let identity = TlsIdentity {
            spki,
            not_after: leaf_not_after.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            generation,
            sans: wanted.names.clone(),
            ca_created,
            leaf_reissued,
        };
        // A root that was rotated by an older build (or restored from a backup
        // with a stale counter) must still move the generation forward, because
        // devices compare it to decide whether to re-confirm the fingerprint.
        if !ca_created {
            generation = match self.read() {
                Ok(previous) if previous.spki == identity.spki => previous.generation.max(generation),
                _ => generation + 1,
            };
        }
        let identity = TlsIdentity { generation, ..identity };
        self.write_identity(&identity)?;
        if leaf_reissued || !self.directory.join(LEAF_CERT).exists() {
            self.write_leaf(&leaf_key, &root_key, &root_pem, &wanted)?;
        }
        Ok(identity)
    }

    fn load_root(&self) -> Result<Option<(KeyPair, String, u32)>, CertError> {
        let (cert_path, key_path) = (self.directory.join(CA_CERT), self.directory.join(CA_KEY));
        if !cert_path.exists() || !key_path.exists() {
            return Ok(None);
        }
        let pem = fs::read_to_string(&cert_path)?;
        let key_pem = fs::read_to_string(&key_path)?;
        let key = match KeyPair::from_pem(&key_pem) {
            Ok(key) => key,
            Err(_) => return Ok(None), // unreadable key: treat as absent, mint a new root
        };
        let generation = self.read().map(|identity| identity.generation).unwrap_or(1);
        Ok(Some((key, pem, generation)))
    }

    fn mint_root(&self) -> Result<(KeyPair, String), CertError> {
        let key = KeyPair::generate()?;
        let mut params = CertificateParams::new(Vec::new())?;
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        params.distinguished_name.push(DnType::CommonName, "AICC Bridge Root");
        params.distinguished_name.push(DnType::OrganizationName, "AICC");
        params.not_before = stamp(chrono::Utc::now() - chrono::Duration::days(1));
        params.not_after = stamp(chrono::Utc::now() + chrono::Duration::days(ROOT_LIFETIME_DAYS));
        params.serial_number = Some(serial());
        let certificate = params.self_signed(&key)?;
        let pem = certificate.pem();
        self.write_private(&self.directory.join(CA_KEY), key.serialize_pem().as_bytes())?;
        write_atomic(&self.directory.join(CA_CERT), pem.as_bytes())?;
        Ok((key, pem))
    }

    fn load_or_mint_leaf(
        &self,
        root_key: &KeyPair,
        root_pem: &str,
        wanted: &SanSet,
    ) -> Result<(KeyPair, bool, chrono::DateTime<chrono::Utc>), CertError> {
        let leaf_key_path = self.directory.join(LEAF_KEY);
        let existing_key = fs::read_to_string(&leaf_key_path).ok().and_then(|pem| KeyPair::from_pem(&pem).ok());
        let reuse_key = existing_key.unwrap_or(KeyPair::generate()?);

        let needs_issue = match self.read() {
            Ok(identity) => {
                let expires = chrono::DateTime::parse_from_rfc3339(&identity.not_after)
                    .map(|value| value.with_timezone(&chrono::Utc))
                    .ok();
                let soon = expires
                    .map(|value| value - chrono::Utc::now() < chrono::Duration::days(LEAF_RENEW_BEFORE_DAYS))
                    .unwrap_or(true);
                soon || !covers(&identity.sans, &wanted.names) || !self.directory.join(LEAF_CERT).exists()
            }
            Err(_) => true,
        };

        let not_after = chrono::Utc::now() + chrono::Duration::days(LEAF_LIFETIME_DAYS);
        if needs_issue {
            self.write_leaf(&reuse_key, root_key, root_pem, wanted)?;
        }
        Ok((reuse_key, needs_issue, not_after))
    }

    fn write_leaf(
        &self,
        leaf_key: &KeyPair,
        root_key: &KeyPair,
        root_pem: &str,
        wanted: &SanSet,
    ) -> Result<(), CertError> {
        let mut params = CertificateParams::new(Vec::new())?;
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature, KeyUsagePurpose::KeyEncipherment];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.distinguished_name.push(DnType::CommonName, "AICC Bridge");
        params.distinguished_name.push(DnType::OrganizationName, "AICC");
        params.subject_alt_names = wanted
            .names
            .iter()
            .map(|name| match name.parse::<IpAddr>() {
                Ok(address) => SanType::IpAddress(address),
                Err(_) => SanType::DnsName(name.clone().try_into().expect("san")),
            })
            .collect();
        params.not_before = stamp(chrono::Utc::now() - chrono::Duration::days(1));
        params.not_after = stamp(chrono::Utc::now() + chrono::Duration::days(LEAF_LIFETIME_DAYS));
        params.serial_number = Some(serial());

        // Take the issuer's subject from the root certificate on disk: minting
        // a fresh distinguished name here would produce an authority key
        // identifier that no longer matches the root, and the device would
        // reject a chain it cannot build.
        let issuer = Issuer::from_ca_cert_pem(root_pem, root_key)?;
        let certificate = params.signed_by(leaf_key, &issuer)?;
        let chain = format!("{}{}", certificate.pem(), root_pem);
        write_atomic(&self.directory.join(LEAF_CERT), chain.as_bytes())?;
        self.write_private(&self.directory.join(LEAF_KEY), leaf_key.serialize_pem().as_bytes())?;
        Ok(())
    }

    fn write_identity(&self, identity: &TlsIdentity) -> Result<(), CertError> {
        let document = serde_json::json!({
            "version": 1,
            "spki": identity.spki,
            "not_after": identity.not_after,
            "generation": identity.generation,
            "sans": identity.sans,
        });
        write_atomic(&self.directory.join(IDENTITY), document.to_string().as_bytes())
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
    wanted.iter().all(|name| existing.iter().any(|have| have == name))
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
        assert!(identity.ca_created, "a fresh directory has no root to reuse");
        assert_eq!(identity.generation, 1);
        assert_eq!(identity.spki.len(), 44, "base64 of a SHA-256 is 44 characters");
        for name in [CA_CERT, CA_KEY, LEAF_CERT, LEAF_KEY, IDENTITY] {
            assert!(store.directory().join(name).exists(), "{name} missing");
        }
        assert_eq!(mode(store.directory()), 0o700, "certificate directory must be private");
        assert_eq!(mode(&store.directory().join(CA_KEY)), 0o600);
        assert_eq!(mode(&store.directory().join(LEAF_KEY)), 0o600);
    }

    #[test]
    fn relaunch_reuses_the_same_identity() {
        let (_keep, store) = store();
        let first = store.ensure(&[]).expect("ensure");
        let second = store.ensure(&[]).expect("ensure again");
        assert!(!second.ca_created, "the root must survive a relaunch");
        assert!(!second.leaf_reissued, "a valid leaf must not be re-issued on every launch");
        assert_eq!(first.spki, second.spki);
        assert_eq!(first.generation, second.generation);
    }

    #[test]
    fn the_leaf_chains_to_the_pinned_root() {
        let (_keep, store) = store();
        let identity = store.ensure(&[]).expect("ensure");
        let chain = fs::read_to_string(store.directory().join(LEAF_CERT)).expect("chain");
        let leaf = chain.split("-----END CERTIFICATE-----").next().expect("leaf").to_owned()
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
        let pinned = pin(&KeyPair::from_pem(&fs::read_to_string(store.directory().join(CA_KEY)).unwrap()).unwrap());
        assert_eq!(pinned, identity.spki, "the published pin must be the root's SPKI");
    }

    #[test]
    fn a_leaf_near_expiry_is_re_issued_on_the_same_root() {
        let (_keep, store) = store();
        let first = store.ensure(&[]).expect("ensure");
        let path = store.directory().join(IDENTITY);
        let mut document: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        document["not_after"] = serde_json::Value::String(
            (chrono::Utc::now() + chrono::Duration::days(10)).format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        );
        fs::write(&path, document.to_string()).unwrap();
        let second = store.ensure(&[]).expect("ensure");
        assert!(second.leaf_reissued, "a leaf inside the renewal window must be re-issued");
        assert_eq!(second.spki, first.spki, "renewal must not change the pin");
        assert_eq!(second.generation, first.generation);
    }

    #[test]
    fn a_new_address_re_issues_the_leaf_without_touching_the_pin() {
        let (_keep, store) = store();
        let first = store.ensure(&[]).expect("ensure");
        let second = store.ensure(&["192.0.2.55".to_string()]).expect("ensure");
        assert!(second.leaf_reissued, "a SAN the leaf does not cover must re-issue it");
        assert!(second.sans.iter().any(|name| name == "192.0.2.55"));
        assert_eq!(second.spki, first.spki);
        let third = store.ensure(&["192.0.2.55".to_string()]).expect("ensure");
        assert!(!third.leaf_reissued, "the same address must not re-issue twice");
    }

    #[test]
    fn reset_rotates_the_root_and_forces_a_new_confirmation() {
        let (_keep, store) = store();
        let first = store.ensure(&[]).expect("ensure");
        let rotated = store.reset().expect("reset");
        assert!(rotated.ca_created);
        assert_ne!(rotated.spki, first.spki, "reset must produce a different root");
        assert!(rotated.generation > first.generation, "devices compare the generation");
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
            let store = TlsStore::new(store.directory().to_path_buf());
            handles.push(std::thread::spawn(move || store.ensure(&[]).expect("ensure").spki));
        }
        let pins: Vec<String> = handles.into_iter().map(|handle| handle.join().unwrap()).collect();
        assert!(
            pins.windows(2).all(|pair| pair[0] == pair[1]),
            "two workers minted different roots: {pins:?}"
        );
    }
}
