use super::*;
use std::io::{Read, Write};

/// Open descriptors created from native picker choices. Paths and bytes are never
/// accepted from a WebView. Pinning each file avoids reopening a replaced path.
pub struct PreparedImport {
    model_id: String,
    bundle_digest: String,
    files: Vec<std::fs::File>,
}

impl ModelManager {
    /// Run on a blocking thread: external disks may take time even to open files.
    pub fn prepare_import(&self, model_id: &str, paths: Vec<PathBuf>) -> Result<PreparedImport> {
        let model = self.0.catalog.model(model_id)?;
        if paths.len() != model.artifacts.len() {
            return Err(E::ImportSelection);
        }
        let mut selected = HashMap::new();
        for path in paths {
            if !path.is_absolute() {
                return Err(E::ImportSelection);
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or(E::ImportSelection)?
                .to_owned();
            if !model.artifacts.iter().any(|a| a.filename == name)
                || selected.insert(name, path).is_some()
            {
                return Err(E::ImportSelection);
            }
        }
        let mut files = Vec::with_capacity(model.artifacts.len());
        for artifact in &model.artifacts {
            let path = selected
                .remove(&artifact.filename)
                .ok_or(E::ImportSelection)?;
            let file = fs::open_file(&path, false)?;
            if file.metadata()?.len() != artifact.size_bytes {
                return Err(E::SizeMismatch);
            }
            files.push(file);
        }
        Ok(PreparedImport {
            model_id: model_id.into(),
            bundle_digest: model.bundle_digest(),
            files,
        })
    }

    pub fn start_import(&self, model_id: &str, source: PreparedImport) -> Result<Uuid> {
        let model = self.0.catalog.model(model_id)?.clone();
        if source.model_id != model_id || source.bundle_digest != model.bundle_digest() {
            return Err(E::ImportSelection);
        }
        let (id, cancel) = self.reserve(&model, true, JobKind::Import)?;
        self.row(model_id, InstallState::Importing, 0, 0, None);
        self.spawn_install(model, id, cancel, Some(source));
        Ok(id)
    }

    pub(super) async fn import_to(
        &self,
        model: &Model,
        id: Uuid,
        cancel: &CancellationToken,
        staging: &Path,
        source: PreparedImport,
    ) -> Result<()> {
        let owner = self.clone();
        let task_model = model.clone();
        let directory = staging.to_path_buf();
        let cancelled = cancel.clone();
        // One blocking copy owns the slot until it returns, even after cancel.
        // An external drive stalled in the OS cannot accumulate replacement jobs.
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut buffer = vec![0; 128 * 1024];
            let mut completed = 0;
            let mut notified = Instant::now();
            for (artifact, mut input) in task_model.artifacts.iter().zip(source.files) {
                if cancelled.is_cancelled() {
                    return Err(E::Cancelled);
                }
                let path = directory.join(format!("{}.part", artifact.filename));
                let mut output = fs::open_file(&path, true)?;
                let mut hash = Sha256::new();
                let mut received = 0_u64;
                loop {
                    if cancelled.is_cancelled() {
                        return Err(E::Cancelled);
                    }
                    let count = input.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    received = received.checked_add(count as u64).ok_or(E::SizeMismatch)?;
                    if received > artifact.size_bytes {
                        return Err(E::SizeMismatch);
                    }
                    output.write_all(&buffer[..count])?;
                    hash.update(&buffer[..count]);
                    if (completed == 0 && received == count as u64)
                        || notified.elapsed() >= Duration::from_millis(200)
                    {
                        owner.row(
                            &task_model.installation_id(),
                            InstallState::Importing,
                            completed + received,
                            0,
                            None,
                        );
                        notified = Instant::now();
                    }
                }
                if received != artifact.size_bytes {
                    return Err(E::SizeMismatch);
                }
                if format!("{:x}", hash.finalize()) != artifact.sha256 {
                    return Err(E::HashMismatch);
                }
                output.flush()?;
                output.sync_all()?;
                drop(output);
                if cancelled.is_cancelled() {
                    return Err(E::Cancelled);
                }
                std::fs::rename(path, directory.join(&artifact.filename))?;
                completed += received;
            }
            Ok(())
        })
        .await
        .map_err(|_| E::Storage)??;
        self.commit_staging(model, id, cancel, staging).await
    }
}
