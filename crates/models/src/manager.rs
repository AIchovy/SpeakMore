use crate::{
    catalog::{Artifact, Model, TrustedCatalog},
    filesystem as fs, ModelError as E, Result,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncSeekExt, AsyncWriteExt},
    sync::Notify,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

mod import;
pub use import::PreparedImport;

/// Only the native controller receives paths. This type is never serialized to a WebView.
#[derive(Clone, Debug)]
pub struct InstalledBundle {
    pub model: Model,
    pub directory: PathBuf,
    pub verified_at: i64,
}
/// Implementations run on blocking threads, after the filesystem commit. On failure, valid
/// files remain for reconciliation; the UI must not report Installed before registration.
pub trait Registry: Send + Sync + 'static {
    fn installed(&self, bundle: &InstalledBundle) -> Result<()>;
    fn removed(&self, model_id: &str) -> Result<()>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallState {
    Checking,
    Absent,
    Downloading,
    Importing,
    Retrying,
    Verifying,
    Installing,
    Installed,
    Deleting,
    Invalid,
    DownloadFailed,
    ImportFailed,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportArtifact {
    pub filename: String,
    pub size_bytes: u64,
    pub role: String,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Download,
    Import,
    Delete,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRow {
    pub model_id: String,
    pub display_name: String,
    pub version: Option<String>,
    pub family_model_id: String,
    pub family: String,
    pub dictionary_prompt: bool,
    pub license_label: String,
    pub languages: Vec<String>,
    pub quantization: String,
    pub size_bytes: u64,
    pub state: InstallState,
    pub received_bytes: u64,
    pub attempt: u8,
    pub error: Option<E>,
    pub error_message: Option<String>,
    pub supported: bool,
    pub artifacts: Vec<ImportArtifact>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsSnapshot {
    pub revision: u64,
    pub models: Vec<ModelRow>,
    pub busy: bool,
    pub active_job_id: Option<Uuid>,
    pub job_model_id: Option<String>,
    pub job_kind: Option<JobKind>,
    pub cancellable: bool,
}
struct Job {
    kind: JobKind,
    id: Uuid,
    model_id: String,
    cancel: CancellationToken,
    committing: bool,
    cancellable: bool,
}
/// Identity and change metadata of a fully hashed file (device, inode, size; mtime, ctime).
/// ctime cannot be set back by a process, so any write or replacement forces a new hash.
type Stamp = ([u64; 3], [i64; 4]);
struct Runtime {
    revision: u64,
    rows: BTreeMap<String, ModelRow>,
    initializing: bool,
    initialization_started: bool,
    /// Startup could not make staging safe. Mutations and leases report it instead of Busy.
    failed: Option<E>,
    /// Existing bundles still awaiting their startup hash; a lease may claim its own first.
    pending: Vec<String>,
    /// In-memory only: every launch hashes each bundle at least once.
    verified: HashMap<String, Vec<Stamp>>,
    job: Option<Job>,
    leases: HashMap<String, usize>,
}
struct Inner {
    root: PathBuf,
    catalog: TrustedCatalog,
    registry: Arc<dyn Registry>,
    client: reqwest::Client,
    state: Mutex<Runtime>,
    changed: Notify,
    notify: Arc<dyn Fn(u64) + Send + Sync>,
    _lock: std::fs::File,
    #[cfg(test)]
    test_origin: Option<String>,
    #[cfg(test)]
    retry_delays: [Duration; 2],
    #[cfg(test)]
    full_hashes: std::sync::atomic::AtomicUsize,
}
#[derive(Clone)]
pub struct ModelManager(Arc<Inner>);
/// Hold this through model Loading and Ready and until the supervisor has retired the worker.
/// Releasing it only after unload prevents deletion while native code still maps the weights.
pub struct ModelLease {
    pub bundle: InstalledBundle,
    owner: ModelManager,
}
impl Drop for ModelLease {
    fn drop(&mut self) {
        let mut state = self.owner.0.state.lock().unwrap();
        if let Some(count) = state.leases.get_mut(&self.bundle.model.installation_id()) {
            *count -= 1;
            if *count == 0 {
                state.leases.remove(&self.bundle.model.installation_id());
            }
        }
    }
}
impl ModelManager {
    /// The caller creates the app-owned parent first. No model data is read on this call.
    pub fn open(
        root: PathBuf,
        catalog: TrustedCatalog,
        registry: Arc<dyn Registry>,
        notify: Arc<dyn Fn(u64) + Send + Sync>,
    ) -> Result<Self> {
        Self::open_with_client(root, catalog, registry, notify, client()?)
    }
    fn open_with_client(
        root: PathBuf,
        catalog: TrustedCatalog,
        registry: Arc<dyn Registry>,
        notify: Arc<dyn Fn(u64) + Send + Sync>,
        client: reqwest::Client,
    ) -> Result<Self> {
        if !root.is_absolute() {
            return Err(E::Storage);
        }
        fs::directory(&root)?;
        let root = root.canonicalize()?;
        let lock_path = root.join("manager.lock");
        let lock = match fs::open_file(&lock_path, true) {
            Ok(file) => file,
            Err(_) => {
                let mut options = std::fs::OpenOptions::new();
                options.read(true).write(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
                }
                if !std::fs::symlink_metadata(&lock_path)?.file_type().is_file() {
                    return Err(E::Storage);
                }
                options.open(lock_path)?
            }
        };
        fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| E::Busy)?;
        fs::directory(&root.join("bundles"))?;
        fs::directory(&root.join("staging"))?;
        let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
        let rows = catalog
            .models()
            .iter()
            .map(|m| {
                (
                    m.installation_id(),
                    ModelRow {
                        model_id: m.installation_id(),
                        display_name: m.display_name.clone(),
                        version: m.version.clone(),
                        family_model_id: m.model_id.clone(),
                        family: m.family.clone(),
                        dictionary_prompt: m.capabilities.dictionary_prompt,
                        license_label: m
                            .artifacts
                            .iter()
                            .find(|a| a.role == "model")
                            .map(|a| a.license.clone())
                            .unwrap_or_default(),
                        languages: m.languages.clone(),
                        quantization: m.quantization.clone(),
                        size_bytes: m.size_bytes(),
                        state: InstallState::Checking,
                        received_bytes: 0,
                        attempt: 0,
                        error: None,
                        error_message: None,
                        supported: m.supported(&platform, env!("CARGO_PKG_VERSION")),
                        artifacts: m
                            .artifacts
                            .iter()
                            .map(|a| ImportArtifact {
                                filename: a.filename.clone(),
                                size_bytes: a.size_bytes,
                                role: a.role.clone(),
                            })
                            .collect(),
                    },
                )
            })
            .collect();
        Ok(Self(Arc::new(Inner {
            root,
            catalog,
            registry,
            client,
            changed: Notify::new(),
            notify,
            _lock: lock,
            state: Mutex::new(Runtime {
                revision: 1,
                rows,
                initializing: true,
                initialization_started: false,
                failed: None,
                pending: Vec::new(),
                verified: HashMap::new(),
                job: None,
                leases: HashMap::new(),
            }),
            #[cfg(test)]
            test_origin: None,
            #[cfg(test)]
            retry_delays: [Duration::from_secs(1), Duration::from_secs(3)],
            #[cfg(test)]
            full_hashes: Default::default(),
        })))
    }
    pub fn snapshot(&self) -> ModelsSnapshot {
        let state = self.0.state.lock().unwrap();
        ModelsSnapshot {
            revision: state.revision,
            models: self
                .0
                .catalog
                .models()
                .iter()
                .filter_map(|model| state.rows.get(&model.installation_id()).cloned())
                .collect(),
            busy: state.initializing || state.job.is_some(),
            active_job_id: state.job.as_ref().map(|j| j.id),
            job_model_id: state.job.as_ref().map(|j| j.model_id.clone()),
            job_kind: state.job.as_ref().map(|j| j.kind),
            cancellable: state
                .job
                .as_ref()
                .is_some_and(|j| j.cancellable && !j.committing && !j.cancel.is_cancelled()),
        }
    }
    // Publish the state and its revision under the same lock. Event consumers may
    // receive snapshots out of order; the revision must describe the exact data.
    fn update<T>(&self, change: impl FnOnce(&mut Runtime) -> T) -> T {
        let (result, revision) = {
            let mut state = self.0.state.lock().unwrap();
            let result = change(&mut state);
            state.revision += 1;
            (result, state.revision)
        };
        self.0.changed.notify_waiters();
        (self.0.notify)(revision);
        result
    }
    fn row(&self, id: &str, status: InstallState, received: u64, attempt: u8, error: Option<E>) {
        self.update(|state| {
            if let Some(row) = state.rows.get_mut(id) {
                row.state = status;
                row.received_bytes = received;
                row.attempt = attempt;
                row.error = error;
                row.error_message = error.map(|e| e.to_string());
            }
        });
    }
    fn path(&self, model: &Model) -> PathBuf {
        self.0
            .root
            .join("bundles")
            .join(&model.model_id)
            .join(model.bundle_digest())
    }
    /// Kept after a resumable download failure; see `install`.
    fn resume_staging(&self, model: &Model) -> PathBuf {
        self.0
            .root
            .join("staging")
            .join(format!("download-{}", model.bundle_digest()))
    }
    /// Revalidate hashes after every launch; reconcile committed-but-unregistered bundles.
    /// This is called once on a background task. Mutations wait only for staging cleanup;
    /// each existing bundle stays Checking (and unavailable) until its own hash completes.
    pub async fn initialize(&self) -> Result<()> {
        {
            let mut state = self.0.state.lock().unwrap();
            if state.initialization_started {
                return Err(E::Busy);
            }
            state.initialization_started = true;
        }
        let owner = self.clone();
        let cleanup = tokio::task::spawn_blocking(move || -> Result<()> {
            for entry in std::fs::read_dir(owner.0.root.join("staging"))? {
                let entry = entry?;
                let name = entry.file_name();
                let name = name.to_str().unwrap_or_default();
                // Partial downloads are resumable only within one run of the app.
                if name
                    .strip_prefix("job-")
                    .is_some_and(|n| Uuid::parse_str(n).is_ok())
                    || name
                        .strip_prefix("download-")
                        .is_some_and(|n| n.len() == 64 && n.bytes().all(|b| b.is_ascii_hexdigit()))
                {
                    fs::remove_bundle(&entry.path())?;
                }
            }
            Ok(())
        })
        .await
        .unwrap_or(Err(E::Storage));
        if let Err(error) = cleanup {
            return self.initialization_error(error);
        }
        let mut pending = Vec::new();
        for model in self.0.catalog.models() {
            if self.path(model).try_exists().unwrap_or(true) {
                pending.push(model.installation_id());
            } else {
                // Absent bundles need no hash: only reconcile their registration.
                self.check(model).await;
            }
        }
        self.update(|state| {
            state.pending = pending;
            state.initializing = false;
        });
        while let Some(id) = self.claim(None) {
            self.check(self.0.catalog.model(&id)?).await;
        }
        // A lease may still be verifying the bundle it claimed.
        loop {
            let changed = self.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if !self
                .0
                .state
                .lock()
                .unwrap()
                .rows
                .values()
                .any(|row| row.state == InstallState::Checking)
            {
                return Ok(());
            }
            changed.await;
        }
    }
    fn claim(&self, id: Option<&str>) -> Option<String> {
        let mut state = self.0.state.lock().unwrap();
        let index = match id {
            Some(id) => state.pending.iter().position(|p| p == id)?,
            None if state.pending.is_empty() => return None,
            None => 0,
        };
        Some(state.pending.remove(index))
    }
    /// Hash one bundle claimed from `pending`, reconcile registration and publish its row.
    async fn check(&self, model: &Model) {
        let id = model.installation_id();
        let status = match self.verify_bundle(model).await {
            Ok(bundle) => self.register(bundle).await.map(|_| InstallState::Installed),
            Err(E::NotInstalled) => self
                .unregister(id.clone())
                .await
                .map(|_| InstallState::Absent),
            Err(error) => self.unregister(id.clone()).await.and(Err(error)),
        };
        match status {
            Ok(status) => self.row(
                &id,
                status,
                if status == InstallState::Installed {
                    model.size_bytes()
                } else {
                    0
                },
                0,
                None,
            ),
            Err(error) => self.row(&id, InstallState::Invalid, 0, 0, Some(error)),
        }
    }
    fn initialization_error(&self, error: E) -> Result<()> {
        self.update(|state| {
            for row in state.rows.values_mut() {
                row.state = InstallState::Invalid;
                row.error = Some(error);
                row.error_message = Some(error.to_string());
            }
            // Remain disabled: unsafe staging must not be ignored by an ensuing download.
            // Report why instead of an endless busy state.
            state.failed = Some(error);
            state.initializing = false;
        });
        Err(error)
    }
    fn reserve(
        &self,
        model: &Model,
        cancellable: bool,
        kind: JobKind,
    ) -> Result<(Uuid, CancellationToken)> {
        self.update(|state| {
            if let Some(error) = state.failed {
                return Err(error);
            }
            let row = &state.rows[&model.installation_id()];
            if state.initializing || state.job.is_some() || row.state == InstallState::Checking {
                return Err(E::Busy);
            }
            if state.leases.contains_key(&model.installation_id())
                || (cancellable && row.state == InstallState::Installed)
            {
                return Err(E::InUse);
            }
            // Retained catalog entries stay removable after they become unsupported.
            if !matches!(kind, JobKind::Delete) && !row.supported {
                return Err(E::UnsupportedModel);
            }
            let id = Uuid::new_v4();
            let cancel = CancellationToken::new();
            state.job = Some(Job {
                kind,
                id,
                model_id: model.installation_id(),
                cancel: cancel.clone(),
                committing: false,
                cancellable,
            });
            Ok((id, cancel))
        })
    }
    fn finish(&self, id: Uuid) {
        self.update(|state| {
            if state.job.as_ref().is_some_and(|j| j.id == id) {
                state.job = None;
            }
        });
    }
    pub fn start_download(&self, model_id: &str) -> Result<Uuid> {
        let model = self.0.catalog.model(model_id)?.clone();
        let (id, cancel) = self.reserve(&model, true, JobKind::Download)?;
        self.row(model_id, InstallState::Downloading, 0, 1, None);
        self.spawn_install(model, id, cancel, None);
        Ok(id)
    }
    fn spawn_install(
        &self,
        model: Model,
        id: Uuid,
        cancel: CancellationToken,
        source: Option<PreparedImport>,
    ) {
        let owner = self.clone();
        let importing = source.is_some();
        tokio::spawn(async move {
            let result = owner.install(&model, id, &cancel, source).await;
            if result.is_ok() {
                // Still in the job slot: no download is using this bundle's resume data.
                let resume = owner.resume_staging(&model);
                let _ = tokio::task::spawn_blocking(move || discard_resume(&resume)).await;
            }
            match result {
                Ok(()) => owner.row(
                    &model.installation_id(),
                    InstallState::Installed,
                    model.size_bytes(),
                    0,
                    None,
                ),
                Err(E::Cancelled) => {
                    owner.row(&model.installation_id(), InstallState::Absent, 0, 0, None)
                }
                Err(error) => owner.row(
                    &model.installation_id(),
                    if matches!(error, E::HashMismatch | E::SizeMismatch | E::Registration) {
                        InstallState::Invalid
                    } else if importing {
                        InstallState::ImportFailed
                    } else {
                        InstallState::DownloadFailed
                    },
                    0,
                    0,
                    Some(error),
                ),
            }
            owner.finish(id);
        });
    }
    pub fn cancel_download(&self, job_id: Uuid) -> Result<()> {
        self.update(|state| {
            let Some(job) = state.job.as_ref().filter(|j| j.id == job_id) else {
                return Ok(());
            };
            if job.committing || !job.cancellable {
                return Err(E::Committing);
            }
            job.cancel.cancel();
            Ok(())
        })
    }
    async fn install(
        &self,
        model: &Model,
        id: Uuid,
        cancel: &CancellationToken,
        source: Option<PreparedImport>,
    ) -> Result<()> {
        // A previous crash may have committed files without recording them. Reuse only after a full hash check.
        if source.is_none() {
            if let Ok(bundle) = self.verify_bundle_cancel(model, cancel).await {
                self.begin_commit(id, cancel)?;
                return self.register(bundle).await;
            }
        }
        // A failed download keeps its verified files and flushed partial bytes here, so an
        // explicit retry in this run resumes. The final commit still hashes every file.
        let staging = match source {
            Some(_) => self.0.root.join("staging").join(format!("job-{id}")),
            None => self.resume_staging(model),
        };
        let root = self.0.root.clone();
        let bytes = model.size_bytes();
        let present = staging.clone();
        tokio::task::spawn_blocking(move || {
            let kept = std::fs::read_dir(&present).map_or(0, |entries| {
                entries
                    .filter_map(|e| e.ok()?.metadata().ok())
                    .filter(|m| m.is_file())
                    .map(|m| m.len())
                    .sum::<u64>()
            });
            fs::check_space(fs2::available_space(root)?, bytes.saturating_sub(kept))
        })
        .await
        .map_err(|_| E::Storage)??;
        if cancel.is_cancelled() {
            return Err(E::Cancelled);
        }
        fs::directory(&staging)?;
        let resumable = source.is_none();
        let result = match source {
            Some(source) => self.import_to(model, id, cancel, &staging, source).await,
            None => self.download_to(model, id, cancel, &staging).await,
        };
        let keep = resumable
            && matches!(
                result,
                Err(E::Network | E::Server | E::HttpStatus | E::InsufficientSpace)
            );
        // Otherwise the staging directory is either renamed or removed before the
        // single-task slot is released.
        let remaining = staging.clone();
        let cleanup = tokio::task::spawn_blocking(move || -> Result<()> {
            if remaining.try_exists()?
                && (!keep
                    || !std::fs::read_dir(&remaining)?
                        .any(|e| e.and_then(|e| e.metadata()).is_ok_and(|m| m.len() > 0)))
            {
                fs::remove_bundle(&remaining)?;
            }
            Ok(())
        })
        .await
        .map_err(|_| E::Storage)?;
        cleanup?;
        result
    }
    async fn download_to(
        &self,
        model: &Model,
        id: Uuid,
        cancel: &CancellationToken,
        staging: &Path,
    ) -> Result<()> {
        let mut completed = 0;
        for file in &model.artifacts {
            let done = staging.join(&file.filename);
            // Renamed only after its hash matched and it was synced by an earlier attempt.
            if tokio::fs::symlink_metadata(&done)
                .await
                .is_ok_and(|m| m.is_file() && m.len() == file.size_bytes)
            {
                completed += file.size_bytes;
                continue;
            }
            let target = staging.join(format!("{}.part", file.filename));
            for attempt in 1..=3 {
                self.row(
                    &model.installation_id(),
                    InstallState::Downloading,
                    completed,
                    attempt,
                    None,
                );
                let result = self
                    .download_file(model, file, &target, completed, attempt, cancel)
                    .await;
                match result {
                    Ok(()) => break,
                    Err(error) => {
                        // Transient failures keep flushed partial bytes for a Range resume.
                        if !error.automatic_retry() || attempt == 3 {
                            return Err(error);
                        }
                        self.row(
                            &model.installation_id(),
                            InstallState::Retrying,
                            completed,
                            attempt + 1,
                            None,
                        );
                        #[cfg(not(test))]
                        let delays = [Duration::from_secs(1), Duration::from_secs(3)];
                        #[cfg(test)]
                        let delays = self.0.retry_delays;
                        tokio::select! { biased; _ = cancel.cancelled() => return Err(E::Cancelled), _ = tokio::time::sleep(delays[usize::from(attempt - 1)]) => {} }
                    }
                }
            }
            tokio::fs::rename(&target, &done).await?;
            completed += file.size_bytes;
        }
        self.commit_staging(model, id, cancel, staging).await
    }
    async fn commit_staging(
        &self,
        model: &Model,
        id: Uuid,
        cancel: &CancellationToken,
        staging: &Path,
    ) -> Result<()> {
        let completed = model.size_bytes();
        self.row(
            &model.installation_id(),
            InstallState::Verifying,
            completed,
            0,
            None,
        );
        // Verify disk bytes (not just the network stream) before the atomic directory commit.
        let stamps = {
            let (model, staging, cancel) = (model.clone(), staging.to_path_buf(), cancel.clone());
            tokio::task::spawn_blocking(move || verify_files(&model, &staging, &cancel))
                .await
                .map_err(|_| E::Storage)??
        };
        self.begin_commit(id, cancel)?;
        let installation = model.installation_id();
        self.0.state.lock().unwrap().verified.remove(&installation);
        self.row(
            &model.installation_id(),
            InstallState::Installing,
            completed,
            0,
            None,
        );
        let owner = self.clone();
        let model = model.clone();
        let staging = staging.to_path_buf();
        let bundle = tokio::task::spawn_blocking(move || -> Result<InstalledBundle> {
            fs::sync_directory(&staging)?;
            let destination = owner.path(&model);
            let parent = destination.parent().ok_or(E::Storage)?;
            fs::directory(parent)?;
            if destination.try_exists()? {
                // A corrupt, unleased bundle is quarantined by rename before replacing it.
                // We never truncate files in an existing installation.
                fs::ordinary_directory(&destination)?;
                let quarantine = owner
                    .0
                    .root
                    .join("staging")
                    .join(format!("job-{}", Uuid::new_v4()));
                std::fs::rename(&destination, &quarantine)?;
                if let Err(error) = std::fs::rename(&staging, &destination) {
                    let _ = std::fs::rename(&quarantine, &destination);
                    return Err(error.into());
                }
                fs::remove_bundle(&quarantine)?;
            } else {
                std::fs::rename(&staging, &destination)?;
            }
            fs::sync_directory(parent)?;
            Ok(InstalledBundle {
                model,
                directory: destination,
                verified_at: timestamp(),
            })
        })
        .await
        .map_err(|_| E::Storage)??;
        // Renaming the directory keeps each file's identity: the next lease needs no re-hash.
        if let Some(stamps) = stamps {
            self.0
                .state
                .lock()
                .unwrap()
                .verified
                .insert(installation, stamps);
        }
        self.register(bundle).await
    }
    fn begin_commit(&self, id: Uuid, cancel: &CancellationToken) -> Result<()> {
        self.update(|state| {
            let job = state.job.as_mut().filter(|j| j.id == id).ok_or(E::Busy)?;
            // Same mutex as cancellation establishes an unambiguous commit boundary.
            if cancel.is_cancelled() {
                return Err(E::Cancelled);
            }
            job.committing = true;
            Ok(())
        })
    }
    async fn download_file(
        &self,
        model: &Model,
        artifact: &Artifact,
        target: &Path,
        completed: u64,
        attempt: u8,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let url = artifact.url.clone();
        #[cfg(test)]
        let url = self
            .0
            .test_origin
            .as_ref()
            .map_or(url, |origin| format!("{origin}/{}", artifact.filename));
        // Resume from the bytes actually on disk. They are re-read here, so the
        // digest below always covers every byte of the file.
        let (file, mut hash, mut received) = {
            let (target, size, cancel) =
                (target.to_path_buf(), artifact.size_bytes, cancel.clone());
            tokio::task::spawn_blocking(move || -> Result<_> {
                let mut file = fs::open_part(&target)?;
                let (hash, received) = hash_stream(&mut file, size, &cancel)?;
                if received > size {
                    file.set_len(0)?;
                    std::io::Seek::rewind(&mut file)?;
                    return Ok((file, Sha256::new(), 0));
                }
                Ok((file, hash, received))
            })
            .await
            .map_err(|_| E::Storage)??
        };
        let mut file = tokio::fs::File::from_std(file);
        if received == artifact.size_bytes {
            if format!("{:x}", hash.clone().finalize()) == artifact.sha256 {
                file.sync_all().await?;
                return Ok(());
            }
            file.set_len(0).await?;
            file.seek(std::io::SeekFrom::Start(0)).await?;
            (hash, received) = (Sha256::new(), 0);
        }
        let mut request = self
            .0
            .client
            .get(url)
            .header(reqwest::header::ACCEPT_ENCODING, "identity");
        if received > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={received}-"));
        }
        let mut response = tokio::select! { biased; _ = cancel.cancelled() => return Err(E::Cancelled), response = request.send() => response.map_err(|_| E::Network)? };
        let status = response.status();
        if received > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT {
            let expected = format!(
                "bytes {received}-{}/{}",
                artifact.size_bytes - 1,
                artifact.size_bytes
            );
            if response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                != Some(expected.as_str())
            {
                // Never splice an unexpected range: start this file again on the next attempt.
                file.set_len(0).await?;
                return Err(E::Server);
            }
        } else if status == reqwest::StatusCode::OK {
            if received > 0 {
                file.set_len(0).await?;
                file.seek(std::io::SeekFrom::Start(0)).await?;
                (hash, received) = (Sha256::new(), 0);
            }
        } else {
            if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
                file.set_len(0).await?;
            }
            return Err(status_error(status));
        }
        if response
            .headers()
            .contains_key(reqwest::header::CONTENT_ENCODING)
            && response.headers()[reqwest::header::CONTENT_ENCODING] != "identity"
        {
            return Err(E::SizeMismatch);
        }
        if response
            .content_length()
            .is_some_and(|length| length != artifact.size_bytes - received)
        {
            return Err(E::SizeMismatch);
        }
        let mut notified = Instant::now();
        let streamed = async {
            loop {
                let chunk = tokio::select! { biased; _ = cancel.cancelled() => return Err(E::Cancelled), chunk = response.chunk() => chunk.map_err(|_| E::Network)? };
                let Some(chunk) = chunk else {
                    return Ok(());
                };
                received = received
                    .checked_add(chunk.len() as u64)
                    .ok_or(E::SizeMismatch)?;
                if received > artifact.size_bytes {
                    return Err(E::SizeMismatch);
                }
                tokio::select! { biased; _ = cancel.cancelled() => return Err(E::Cancelled), written = file.write_all(&chunk) => written? }
                hash.update(&chunk);
                if notified.elapsed() >= Duration::from_millis(200) {
                    self.row(
                        &model.installation_id(),
                        InstallState::Downloading,
                        completed + received,
                        attempt,
                        None,
                    );
                    notified = Instant::now();
                }
            }
        }
        .await;
        if let Err(error) = streamed {
            // Settle any in-flight write so a retry resumes from what is really on disk.
            let _ = file.flush().await;
            return Err(error);
        }
        if received != artifact.size_bytes {
            return Err(E::SizeMismatch);
        }
        if format!("{:x}", hash.finalize()) != artifact.sha256 {
            return Err(E::HashMismatch);
        }
        file.flush().await?;
        file.sync_all().await?;
        Ok(())
    }
    async fn verify_bundle(&self, model: &Model) -> Result<InstalledBundle> {
        self.verify_bundle_cancel(model, &CancellationToken::new())
            .await
    }
    async fn verify_bundle_cancel(
        &self,
        model: &Model,
        cancel: &CancellationToken,
    ) -> Result<InstalledBundle> {
        let path = self.path(model);
        let id = model.installation_id();
        let cached = self.0.state.lock().unwrap().verified.get(&id).cloned();
        let (task_model, task_path, task_cancel) = (model.clone(), path.clone(), cancel.clone());
        // One blocking task per bundle: metadata checks and hashing never run on async threads.
        let result = tokio::task::spawn_blocking(move || -> Result<_> {
            if !task_path.try_exists()? {
                return Err(E::NotInstalled);
            }
            fs::ordinary_directory(task_path.parent().ok_or(E::Storage)?)?;
            match cached {
                // Same files, unchanged since their full hash in this run: skip re-reading GBs.
                Some(stamps) if unchanged(&task_model, &task_path, &stamps) => {
                    Ok((Some(stamps), false))
                }
                _ => Ok((verify_files(&task_model, &task_path, &task_cancel)?, true)),
            }
        })
        .await
        .map_err(|_| E::Storage)
        .and_then(|r| r);
        let mut state = self.0.state.lock().unwrap();
        match result {
            Ok((Some(stamps), hashed)) => {
                #[cfg(test)]
                if hashed {
                    self.0
                        .full_hashes
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                let _ = hashed;
                state.verified.insert(id, stamps);
            }
            Ok((None, _)) => {
                state.verified.remove(&id);
            }
            Err(error) => {
                state.verified.remove(&id);
                return Err(error);
            }
        }
        drop(state);
        Ok(InstalledBundle {
            model: model.clone(),
            directory: path,
            verified_at: timestamp(),
        })
    }
    async fn register(&self, bundle: InstalledBundle) -> Result<()> {
        let registry = self.0.registry.clone();
        tokio::task::spawn_blocking(move || registry.installed(&bundle))
            .await
            .map_err(|_| E::Registration)?
            .map_err(|_| E::Registration)
    }
    async fn unregister(&self, model_id: String) -> Result<()> {
        let registry = self.0.registry.clone();
        tokio::task::spawn_blocking(move || registry.removed(&model_id))
            .await
            .map_err(|_| E::Registration)?
            .map_err(|_| E::Registration)
    }
    pub async fn lease(&self, model_id: &str) -> Result<ModelLease> {
        let model = self.0.catalog.model(model_id)?.clone();
        let mut checked = false;
        loop {
            let changed = self.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut state = self.0.state.lock().unwrap();
                if let Some(error) = state.failed {
                    return Err(error);
                }
                // Jobs on other bundles never touch this directory.
                if state.initializing || state.job.as_ref().is_some_and(|j| j.model_id == model_id)
                {
                    return Err(E::Busy);
                }
                if !state.rows[model_id].supported {
                    return Err(E::UnsupportedModel);
                }
                match state.rows[model_id].state {
                    InstallState::Installed => {
                        *state.leases.entry(model_id.into()).or_default() += 1;
                        break;
                    }
                    InstallState::Checking => {}
                    // The startup check this lease waited for reports why (e.g. HashMismatch).
                    _ if checked => {
                        return Err(state.rows[model_id].error.unwrap_or(E::NotInstalled))
                    }
                    _ => return Err(E::NotInstalled),
                }
            }
            checked = true;
            if self.claim(Some(model_id)).is_some() {
                // Startup hashing reaches the requested model first. A task owns the
                // check so a dropped caller can never leave the row Checking.
                let (owner, task_model) = (self.clone(), model.clone());
                tokio::spawn(async move { owner.check(&task_model).await })
                    .await
                    .map_err(|_| E::Storage)?;
            } else {
                changed.await;
            }
        }
        let mut provisional = ModelLease {
            bundle: InstalledBundle {
                model: model.clone(),
                directory: self.path(&model),
                verified_at: 0,
            },
            owner: self.clone(),
        };
        match self.verify_bundle(&model).await {
            Ok(bundle) => {
                provisional.bundle = bundle;
                Ok(provisional)
            }
            Err(error) => {
                self.unregister(model_id.into()).await?;
                self.row(model_id, InstallState::Invalid, 0, 0, Some(error));
                Err(error)
            }
        }
    }
    pub async fn delete(&self, model_id: &str) -> Result<()> {
        let model = self.0.catalog.model(model_id)?.clone();
        let (id, _) = self.reserve(&model, false, JobKind::Delete)?;
        self.0.state.lock().unwrap().verified.remove(model_id);
        self.row(model_id, InstallState::Deleting, 0, 0, None);
        let owner = self.clone();
        let model_id = model_id.to_string();
        // Own the operation in a task so cancellation of the IPC await cannot leave a reserved slot.
        tokio::spawn(async move {
            let task_owner = owner.clone();
            let result = tokio::task::spawn_blocking(move || -> Result<()> {
                // Clear registration first. Crash after this point leaves a valid orphan that
                // startup re-verifies, never an installed database row pointing to partial files.
                task_owner.0.registry.removed(&model.installation_id())?;
                let path = task_owner.path(&model);
                if path.try_exists()? {
                    fs::ordinary_directory(path.parent().ok_or(E::Storage)?)?;
                    fs::remove_bundle(&path)?;
                    fs::sync_directory(path.parent().ok_or(E::Storage)?)?;
                }
                discard_resume(&task_owner.resume_staging(&model));
                Ok(())
            })
            .await
            .map_err(|_| E::Storage)
            .and_then(|r| r);
            match result {
                Ok(()) => owner.row(&model_id, InstallState::Absent, 0, 0, None),
                Err(error) => owner.row(&model_id, InstallState::Invalid, 0, 0, Some(error)),
            }
            owner.finish(id);
            result
        })
        .await
        .map_err(|_| E::Storage)?
    }
}
/// Resume data is useless once its bundle is installed or deleted. Best effort:
/// startup cleanup still removes anything left behind.
fn discard_resume(path: &Path) {
    if path.try_exists().unwrap_or(false) {
        let _ = fs::remove_bundle(path);
    }
}
fn timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
/// Run on a blocking thread. Returns the files' stamps when they did not change while
/// being hashed (unix only); otherwise nothing is cached and the next use hashes again.
fn verify_files(
    model: &Model,
    path: &Path,
    cancel: &CancellationToken,
) -> Result<Option<Vec<Stamp>>> {
    fs::ordinary_directory(path)?;
    expected_entries(model, path)?;
    let mut stamps = Some(Vec::with_capacity(model.artifacts.len()));
    for artifact in &model.artifacts {
        let mut file = fs::open_file(&path.join(&artifact.filename), false)?;
        let before = file.metadata()?;
        if before.len() != artifact.size_bytes {
            return Err(E::SizeMismatch);
        }
        let (hash, received) = hash_stream(&mut file, artifact.size_bytes, cancel)?;
        if received != artifact.size_bytes {
            return Err(E::SizeMismatch);
        }
        if format!("{:x}", hash.finalize()) != artifact.sha256 {
            return Err(E::HashMismatch);
        }
        stamps = match (stamps, stamp(&before), stamp(&file.metadata()?)) {
            (Some(mut all), Some(before), Some(after)) if before == after => {
                all.push(before);
                Some(all)
            }
            _ => None,
        };
    }
    Ok(stamps)
}
/// Cheap check that a bundle still holds exactly the files hashed earlier in this run.
fn unchanged(model: &Model, path: &Path, stamps: &[Stamp]) -> bool {
    let check = || -> Result<bool> {
        fs::ordinary_directory(path)?;
        expected_entries(model, path)?;
        for (artifact, expected) in model.artifacts.iter().zip(stamps) {
            let file = fs::open_file(&path.join(&artifact.filename), false)?;
            if stamp(&file.metadata()?).as_ref() != Some(expected) {
                return Ok(false);
            }
        }
        Ok(stamps.len() == model.artifacts.len())
    };
    check().unwrap_or(false)
}
/// Exactly the catalog files must be present. Finder metadata (.DS_Store, AppleDouble
/// `._*`) is tolerated as plain files; anything else, including links, is rejected.
fn expected_entries(model: &Model, path: &Path) -> Result<()> {
    let mut found = 0;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().unwrap_or_default();
        if model.artifacts.iter().any(|a| a.filename == name) {
            found += 1;
        } else if !((name == ".DS_Store" || name.starts_with("._")) && entry.file_type()?.is_file())
        {
            return Err(E::SizeMismatch);
        }
    }
    if found != model.artifacts.len() {
        return Err(E::SizeMismatch);
    }
    Ok(())
}
/// Hash at most `limit + 1` bytes with a large buffer; the count exceeds `limit` on overflow.
fn hash_stream(
    file: &mut std::fs::File,
    limit: u64,
    cancel: &CancellationToken,
) -> Result<(Sha256, u64)> {
    let mut buffer = vec![0; 2 * 1024 * 1024];
    let mut hash = Sha256::new();
    let mut received = 0_u64;
    loop {
        if cancel.is_cancelled() {
            return Err(E::Cancelled);
        }
        let read = match file.read(&mut buffer) {
            Ok(0) => return Ok((hash, received)),
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        received += read as u64;
        if received > limit {
            return Ok((hash, received));
        }
        hash.update(&buffer[..read]);
    }
}
fn stamp(metadata: &std::fs::Metadata) -> Option<Stamp> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some((
            [metadata.dev(), metadata.ino(), metadata.size()],
            [
                metadata.mtime(),
                metadata.mtime_nsec(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            ],
        ))
    }
    #[cfg(not(unix))]
    {
        // Without a change time that callers cannot reset, always hash again.
        let _ = metadata;
        None
    }
}
/// Rate limits, timeouts and server faults are temporary. Other statuses mean the
/// pinned address cannot serve this file (removed, moved or access denied).
fn status_error(status: reqwest::StatusCode) -> E {
    if status.is_server_error()
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
    {
        E::Server
    } else {
        E::HttpStatus
    }
}
fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(30))
        .no_gzip()
        .no_brotli()
        .no_zstd()
        .no_deflate()
        .referer(false)
        .retry(reqwest::retry::never())
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let url = attempt.url();
            let host = url.host_str().unwrap_or("");
            let allowed = host == "huggingface.co"
                || host.ends_with(".huggingface.co")
                || host == "hf.co"
                || host.ends_with(".hf.co");
            if attempt.previous().len() >= 4
                || url.scheme() != "https"
                || !allowed
                || !url.username().is_empty()
                || url.password().is_some()
                || url.port().is_some()
            {
                attempt.error("model redirect rejected")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|_| E::Network)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
