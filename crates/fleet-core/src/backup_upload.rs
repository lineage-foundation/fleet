//! Best-effort upload of on-disk RocksDB backup directories to an S3-compatible
//! bucket (Cloudflare R2).
//!
//! The node already keeps periodic local backups next to the live DB (see
//! `db_utils::SimpleDb::file_backup`). Those live on the same volume as the DB
//! and do not survive volume loss, so this module ships them off-box.
//!
//! Everything here is best-effort: it must never panic, never block the
//! RAFT/consensus loop, and never propagate an error that would stop block
//! processing. Uploads run in a detached task and failures are logged at warn.

use crate::utils::BackupCheck;
use s3::bucket::Bucket;
use s3::creds::Credentials;
use s3::region::Region;
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::warn;

/// Environment variable names carrying the S3/R2 connection details. Secrets are
/// never read from config/TOML; only these process environment variables.
pub const ENV_BUCKET: &str = "LINEAGE_BACKUP_S3_BUCKET";
pub const ENV_ENDPOINT: &str = "LINEAGE_BACKUP_S3_ENDPOINT";
pub const ENV_REGION: &str = "LINEAGE_BACKUP_S3_REGION";
pub const ENV_ACCESS_KEY_ID: &str = "LINEAGE_BACKUP_S3_ACCESS_KEY_ID";
pub const ENV_SECRET_ACCESS_KEY: &str = "LINEAGE_BACKUP_S3_SECRET_ACCESS_KEY";
pub const ENV_PREFIX: &str = "LINEAGE_BACKUP_S3_PREFIX";

/// Default region for Cloudflare R2, which ignores the region but requires a value.
const DEFAULT_REGION: &str = "auto";

type BoxError = Box<dyn Error + Send + Sync>;

/// Connection details for the S3-compatible bucket that receives backups.
///
/// `Debug` is implemented by hand so credentials are never written to logs.
#[derive(Clone, PartialEq, Eq)]
pub struct S3BackupConfig {
    pub bucket: String,
    pub endpoint: String,
    pub region: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub prefix: Option<String>,
}

impl fmt::Debug for S3BackupConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3BackupConfig")
            .field("bucket", &self.bucket)
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .field("access_key_id", &"<redacted>")
            .field("secret_access_key", &"<redacted>")
            .field("prefix", &self.prefix)
            .finish()
    }
}

/// Number of files uploaded and skipped during a single upload pass.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct UploadStats {
    pub uploaded: usize,
    pub skipped: usize,
    pub failed: usize,
}

impl S3BackupConfig {
    /// Pure constructor so callers/tests can build a config without touching the
    /// process environment.
    pub fn new(
        bucket: String,
        endpoint: String,
        region: String,
        access_key_id: String,
        secret_access_key: String,
        prefix: Option<String>,
    ) -> Self {
        Self {
            bucket,
            endpoint,
            region,
            access_key_id,
            secret_access_key,
            prefix,
        }
    }

    /// Build a config from process environment variables. Returns `Some` only
    /// when bucket, endpoint, access key id, and secret access key are all
    /// present and non-empty; otherwise `None`.
    pub fn from_env() -> Option<Self> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Build a config from an arbitrary key lookup. Empty values are treated as
    /// absent so a blank env var does not enable a broken upload. Split out from
    /// [`Self::from_env`] to keep the gating logic unit-testable without mutating
    /// global process env.
    pub fn from_lookup<F>(lookup: F) -> Option<Self>
    where
        F: Fn(&str) -> Option<String>,
    {
        let required = |key: &str| lookup(key).filter(|v| !v.is_empty());

        let bucket = required(ENV_BUCKET)?;
        let endpoint = required(ENV_ENDPOINT)?;
        let access_key_id = required(ENV_ACCESS_KEY_ID)?;
        let secret_access_key = required(ENV_SECRET_ACCESS_KEY)?;
        let region = required(ENV_REGION).unwrap_or_else(|| DEFAULT_REGION.to_string());
        let prefix = required(ENV_PREFIX);

        Some(Self::new(
            bucket,
            endpoint,
            region,
            access_key_id,
            secret_access_key,
            prefix,
        ))
    }

    /// Construct a path-style bucket handle for the custom S3-compatible endpoint.
    /// R2 and most non-AWS providers need path-style addressing.
    fn open_bucket(&self) -> Result<Box<Bucket>, BoxError> {
        let region = Region::Custom {
            region: self.region.clone(),
            endpoint: self.endpoint.clone(),
        };
        let credentials = Credentials::new(
            Some(&self.access_key_id),
            Some(&self.secret_access_key),
            None,
            None,
            None,
        )?;
        Ok(Bucket::new(&self.bucket, region, credentials)?.with_path_style())
    }
}

/// Build the S3 object key for a file within the backup directory.
///
/// The layout is `{prefix}/{key_prefix}/{relative_path}`; the prefix segment is
/// omitted when no prefix is configured.
pub fn build_key(prefix: Option<&str>, key_prefix: &str, relative_path: &str) -> String {
    let relative_path = relative_path.trim_start_matches('/');
    match prefix {
        Some(prefix) if !prefix.is_empty() => {
            format!("{}/{}/{}", prefix.trim_matches('/'), key_prefix, relative_path)
        }
        _ => format!("{}/{}", key_prefix, relative_path),
    }
}

/// Recursively collect every file (not directory) under `dir` as absolute paths.
fn collect_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    Ok(files)
}

/// Upload every file under `local_backup_dir` to the bucket, skipping objects
/// that already exist with a matching content-length.
///
/// A HEAD request (cheap, Class B on R2) decides whether a PUT (Class A) is
/// needed: RocksDB BackupEngine SST files are immutable, so only small metadata
/// files ever change and get re-uploaded. Per-file failures are counted and
/// logged, not propagated, so one bad object cannot abort the pass; the returned
/// `Err` is reserved for setup failures (e.g. bad credentials).
pub async fn upload_backup_dir(
    cfg: &S3BackupConfig,
    local_backup_dir: &Path,
    key_prefix: &str,
) -> Result<UploadStats, BoxError> {
    let bucket = cfg.open_bucket()?;
    let files = collect_files(local_backup_dir)?;

    let mut stats = UploadStats::default();
    for path in files {
        let relative = match path.strip_prefix(local_backup_dir) {
            Ok(relative) => relative,
            Err(_) => &path,
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        let key = build_key(cfg.prefix.as_deref(), key_prefix, &relative);

        let local_len = match std::fs::metadata(&path) {
            Ok(meta) => meta.len(),
            Err(e) => {
                warn!("Backup upload: cannot stat {}: {}", path.display(), e);
                stats.failed += 1;
                continue;
            }
        };

        if !remote_needs_upload(&bucket, &key, local_len).await {
            stats.skipped += 1;
            continue;
        }

        match upload_file(&bucket, &path, &key).await {
            Ok(()) => stats.uploaded += 1,
            Err(e) => {
                warn!("Backup upload: failed to put {key}: {e}");
                stats.failed += 1;
            }
        }
    }

    Ok(stats)
}

/// Decide whether an object must be uploaded: yes when it is missing or its
/// remote content-length differs from the local file size. Any HEAD error is
/// treated as "missing" so we err towards uploading rather than skipping.
async fn remote_needs_upload(bucket: &Bucket, key: &str, local_len: u64) -> bool {
    match bucket.head_object(key).await {
        Ok((head, 200)) => head.content_length != Some(local_len as i64),
        Ok(_) => true,
        Err(_) => true,
    }
}

/// Stream a single local file to the bucket under `key`.
async fn upload_file(bucket: &Bucket, path: &Path, key: &str) -> Result<(), BoxError> {
    let mut file = tokio::fs::File::open(path).await?;
    bucket.put_object_stream(&mut file, key).await?;
    Ok(())
}

/// Wires the S3 config and upload cadence into a node's backup path. Owns the
/// "upload in flight" guard so a slow upload can never pile up behind the next
/// backup.
pub struct BackupUploader {
    cfg: Option<S3BackupConfig>,
    upload_check: BackupCheck,
    key_prefix: String,
    in_flight: Arc<AtomicBool>,
}

impl fmt::Debug for BackupUploader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BackupUploader")
            .field("enabled", &self.cfg.is_some())
            .field("key_prefix", &self.key_prefix)
            .finish()
    }
}

impl BackupUploader {
    /// Build from process env. `key_prefix` namespaces this node's objects
    /// (e.g. `"storage-0"`). `upload_modulo` gates the cadence; when it is set
    /// but the S3 env is incomplete a warning is logged and uploads stay off.
    pub fn from_env(key_prefix: String, upload_modulo: Option<u64>) -> Self {
        let cfg = S3BackupConfig::from_env();
        if upload_modulo.is_some() && cfg.is_none() {
            warn!(
                "backup_upload_modulo is set but the S3 backup environment is incomplete; \
                 on-disk backups will NOT be uploaded (need {ENV_BUCKET}, {ENV_ENDPOINT}, \
                 {ENV_ACCESS_KEY_ID}, {ENV_SECRET_ACCESS_KEY})"
            );
        }
        Self {
            cfg,
            upload_check: BackupCheck::new(upload_modulo),
            key_prefix,
            in_flight: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Whether an upload should fire for the given block number: the cadence
    /// matches and an S3 config is available.
    pub fn need_upload(&self, b_num: u64) -> bool {
        self.cfg.is_some() && self.upload_check.need_backup(b_num)
    }

    /// Spawn a detached, best-effort upload of `local_backup_dir`. Returns
    /// immediately; never blocks or panics. Skips if a previous upload is still
    /// running so uploads cannot overlap.
    pub fn spawn_upload(&self, local_backup_dir: PathBuf) {
        let cfg = match &self.cfg {
            Some(cfg) => cfg.clone(),
            None => return,
        };

        if self
            .in_flight
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            warn!(
                "Skipping backup upload for {}: a previous upload is still in flight",
                self.key_prefix
            );
            return;
        }

        let in_flight = self.in_flight.clone();
        let key_prefix = self.key_prefix.clone();
        tokio::spawn(async move {
            // Release the single-flight flag on every exit path, including a
            // panic or cancellation inside the upload, so a failed upload can
            // never wedge this node's uploads permanently "in flight".
            struct InFlightGuard(Arc<AtomicBool>);
            impl Drop for InFlightGuard {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::SeqCst);
                }
            }
            let _guard = InFlightGuard(in_flight);

            match upload_backup_dir(&cfg, &local_backup_dir, &key_prefix).await {
                Ok(stats) => warn!(
                    "Backup upload {key_prefix}: uploaded={} skipped={} failed={}",
                    stats.uploaded, stats.skipped, stats.failed
                ),
                Err(e) => warn!("Backup upload {key_prefix} aborted: {e}"),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn full_env() -> HashMap<String, String> {
        [
            (ENV_BUCKET, "backups"),
            (ENV_ENDPOINT, "https://acct.r2.cloudflarestorage.com"),
            (ENV_REGION, "auto"),
            (ENV_ACCESS_KEY_ID, "AKIA"),
            (ENV_SECRET_ACCESS_KEY, "secret"),
            (ENV_PREFIX, "fleet"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    fn lookup_from(map: HashMap<String, String>) -> impl Fn(&str) -> Option<String> {
        move |key: &str| map.get(key).cloned()
    }

    #[test]
    fn from_lookup_fully_configured_is_some() {
        let cfg = S3BackupConfig::from_lookup(lookup_from(full_env())).unwrap();
        assert_eq!(cfg.bucket, "backups");
        assert_eq!(cfg.endpoint, "https://acct.r2.cloudflarestorage.com");
        assert_eq!(cfg.region, "auto");
        assert_eq!(cfg.access_key_id, "AKIA");
        assert_eq!(cfg.secret_access_key, "secret");
        assert_eq!(cfg.prefix.as_deref(), Some("fleet"));
    }

    #[test]
    fn from_lookup_defaults_region_and_optional_prefix() {
        let mut env = full_env();
        env.remove(ENV_REGION);
        env.remove(ENV_PREFIX);
        let cfg = S3BackupConfig::from_lookup(lookup_from(env)).unwrap();
        assert_eq!(cfg.region, "auto");
        assert_eq!(cfg.prefix, None);
    }

    #[test]
    fn from_lookup_missing_required_field_is_none() {
        for missing in [
            ENV_BUCKET,
            ENV_ENDPOINT,
            ENV_ACCESS_KEY_ID,
            ENV_SECRET_ACCESS_KEY,
        ] {
            let mut env = full_env();
            env.remove(missing);
            assert!(
                S3BackupConfig::from_lookup(lookup_from(env)).is_none(),
                "expected None when {missing} is absent"
            );
        }
    }

    #[test]
    fn from_lookup_empty_required_field_is_none() {
        let mut env = full_env();
        env.insert(ENV_BUCKET.to_string(), String::new());
        assert!(S3BackupConfig::from_lookup(lookup_from(env)).is_none());
    }

    #[test]
    fn need_upload_modulo_logic() {
        let cfg = S3BackupConfig::from_lookup(lookup_from(full_env()));
        let uploader = BackupUploader {
            cfg,
            upload_check: BackupCheck::new(Some(5)),
            key_prefix: "storage-0".to_string(),
            in_flight: Arc::new(AtomicBool::new(false)),
        };
        assert!(!uploader.need_upload(0));
        assert!(uploader.need_upload(5));
        assert!(uploader.need_upload(10));
        assert!(!uploader.need_upload(7));
    }

    #[test]
    fn need_upload_disabled_when_modulo_none() {
        let cfg = S3BackupConfig::from_lookup(lookup_from(full_env()));
        let uploader = BackupUploader {
            cfg,
            upload_check: BackupCheck::new(None),
            key_prefix: "storage-0".to_string(),
            in_flight: Arc::new(AtomicBool::new(false)),
        };
        assert!(!uploader.need_upload(5));
        assert!(!uploader.need_upload(10));
    }

    #[test]
    fn need_upload_disabled_when_no_s3_config() {
        let uploader = BackupUploader {
            cfg: None,
            upload_check: BackupCheck::new(Some(5)),
            key_prefix: "storage-0".to_string(),
            in_flight: Arc::new(AtomicBool::new(false)),
        };
        assert!(!uploader.need_upload(5));
    }

    #[test]
    fn build_key_with_prefix() {
        assert_eq!(
            build_key(Some("fleet"), "storage-0", "meta/1"),
            "fleet/storage-0/meta/1"
        );
    }

    #[test]
    fn build_key_without_prefix() {
        assert_eq!(build_key(None, "mempool-2", "shared_checksum/x.sst"), "mempool-2/shared_checksum/x.sst");
    }

    #[test]
    fn build_key_trims_separators() {
        assert_eq!(
            build_key(Some("/fleet/"), "storage-1", "/private/00001"),
            "fleet/storage-1/private/00001"
        );
    }

    #[test]
    fn build_key_nested_relative_path() {
        assert_eq!(
            build_key(Some("p"), "storage-0", "a/b/c/00003.sst"),
            "p/storage-0/a/b/c/00003.sst"
        );
    }
}
