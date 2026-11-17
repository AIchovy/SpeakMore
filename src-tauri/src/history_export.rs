use speakmore_core::{
    error::{AppError, Result},
    history_export::{Format, Scope},
    storage::Store,
};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

static EXPORTING: AtomicBool = AtomicBool::new(false);
pub(crate) struct ExportGuard;
impl ExportGuard {
    pub fn acquire() -> Result<Self> {
        EXPORTING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| AppError::new("ExportBusy", "已有导出正在进行，请先完成或取消。", true))
    }
}
impl Drop for ExportGuard {
    fn drop(&mut self) {
        EXPORTING.store(false, Ordering::Release);
    }
}

/// Runs on the single storage thread: render one consistent snapshot in
/// memory (bounded by the export record limit) and leave file I/O to `save`.
pub(crate) fn render(store: &Store, scope: &Scope, format: Format) -> Result<(Vec<u8>, usize)> {
    let mut bytes = Vec::new();
    let count = store.export_history(scope, format, &mut bytes)?;
    Ok((bytes, count))
}
pub(crate) async fn save(path: PathBuf, bytes: Vec<u8>) -> Result<()> {
    use speakmore_core::history_export::io_error;
    tauri::async_runtime::spawn_blocking(move || persist(&path, &bytes))
        .await
        .map_err(|_| io_error(std::io::ErrorKind::Other.into()))?
}
/// Never truncate an existing destination before the complete snapshot is
/// written and synced. No paths come from a WebView command argument.
fn persist(path: &Path, bytes: &[u8]) -> Result<()> {
    use speakmore_core::history_export::io_error;
    let parent = path
        .parent()
        .ok_or_else(|| AppError::invalid("请选择有效的保存位置。"))?;
    if std::fs::symlink_metadata(path).is_ok_and(|m| !m.is_file() || m.file_type().is_symlink()) {
        return Err(AppError::invalid(
            "不能替换文件夹或符号链接，请选择其他文件名。",
        ));
    }
    let mut temp = tempfile::Builder::new()
        .prefix(".speakmore-export-")
        .tempfile_in(parent)
        .map_err(io_error)?;
    temp.as_file_mut().write_all(bytes).map_err(io_error)?;
    temp.as_file().sync_all().map_err(io_error)?;
    temp.persist(path).map_err(|e| io_error(e.error))?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) async fn choose(
    window: &tauri::WebviewWindow,
    format: Format,
) -> Result<Option<std::path::PathBuf>> {
    use objc2::{rc::Retained, MainThreadMarker};
    use objc2_app_kit::{NSModalResponseCancel, NSModalResponseOK, NSSavePanel, NSWindow};
    use objc2_foundation::{NSArray, NSString};
    use std::{cell::RefCell, ffi::CStr, os::unix::ffi::OsStrExt};
    let (tx, rx) = tokio::sync::oneshot::channel();
    let host = window.clone();
    window
        .run_on_main_thread(move || {
            let result = (|| -> Result<()> {
                let mtm = MainThreadMarker::new().ok_or_else(picker_error)?;
                // SAFETY: Tauri provides an NSWindow pointer; retain for this sheet.
                let host = unsafe {
                    Retained::retain(
                        host.ns_window()
                            .map_err(|_| picker_error())?
                            .cast::<NSWindow>(),
                    )
                }
                .ok_or_else(picker_error)?;
                let panel = NSSavePanel::savePanel(mtm);
                let extension = NSString::from_str(format.extension());
                #[allow(deprecated)]
                panel.setAllowedFileTypes(Some(&NSArray::from_slice(&[&*extension])));
                panel.setAllowsOtherFileTypes(false);
                panel.setCanCreateDirectories(true);
                panel.setShowsTagField(false);
                panel.setNameFieldStringValue(&NSString::from_str(&format!(
                    "SpeakMore-history-{}.{}",
                    chrono::Local::now().format("%Y%m%d-%H%M%S"),
                    format.extension()
                )));
                let completion_panel = panel.clone();
                let tx = RefCell::new(Some(tx));
                let callback = block2::RcBlock::new(move |response| {
                    completion_panel.orderOut(None);
                    let result = if response == NSModalResponseCancel {
                        Ok(None)
                    } else if response == NSModalResponseOK {
                        completion_panel
                            .URL()
                            .filter(|url| url.isFileURL())
                            .map(|url| {
                                // SAFETY: NSURL owns a terminated filesystem string; copy before URL drops.
                                let bytes = unsafe {
                                    CStr::from_ptr(url.fileSystemRepresentation().as_ptr())
                                }
                                .to_bytes();
                                std::path::PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
                            })
                            .map(Some)
                            .ok_or_else(picker_error)
                    } else {
                        Err(picker_error())
                    };
                    if let Some(tx) = tx.borrow_mut().take() {
                        let _ = tx.send(result);
                    }
                });
                panel.beginSheetModalForWindow_completionHandler(&host, &callback);
                Ok(())
            })();
            // On setup failure, dropping the captured sender makes rx fail safely.
            let _ = result;
        })
        .map_err(|_| picker_error())?;
    rx.await.map_err(|_| picker_error())?
}
#[cfg(not(target_os = "macos"))]
pub(crate) async fn choose(
    _: &tauri::WebviewWindow,
    _: Format,
) -> Result<Option<std::path::PathBuf>> {
    Err(AppError::new(
        "UnsupportedPlatform",
        "当前构建仅在 macOS 提供历史导出。",
        false,
    ))
}
#[cfg(target_os = "macos")]
fn picker_error() -> AppError {
    AppError::new("SaveDialogUnavailable", "无法打开保存窗口，请重试。", true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use speakmore_core::types::{ResultRecord, SettingsPatch};
    fn write(store: &Store, path: &Path, scope: &Scope, format: Format) -> Result<usize> {
        let (bytes, count) = render(store, scope, format)?;
        persist(path, &bytes)?;
        Ok(count)
    }
    #[test]
    fn failed_export_preserves_destination_and_success_replaces_complete_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("export.csv");
        std::fs::write(&path, "existing file").unwrap();
        let mut store = Store::memory().unwrap();
        let scope = Scope::Filtered {
            query: String::new(),
        };
        assert!(write(&store, &path, &scope, Format::Csv).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "existing file");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        store
            .update_settings(
                1,
                SettingsPatch {
                    history_enabled: Some(true),
                    ..Default::default()
                },
                0,
            )
            .unwrap();
        let r = ResultRecord {
            session_id: uuid::Uuid::new_v4(),
            raw_text: "=1+1,\"quoted\"\n中文".into(),
            polished_text: None,
            model_id: "fixture".into(),
            model_sha256: "a".repeat(64),
            dictionary_revision: 1,
            language: "zh".into(),
            duration_ms: 300_000,
            asr_ms: 1,
            needs_review: true,
            polish_ms: None,
            polish_outcome: "off".into(),
            polish_reason: None,
            delivery_status: "ready_for_copy".into(),
            delivery_reason: None,
            app_identity: None,
            // Export skips rows already past the retention window.
            created_at: crate::now(),
        };
        store
            .persist_generated(&r, "2026-09-15", true, false)
            .unwrap();
        assert_eq!(write(&store, &path, &scope, Format::Csv).unwrap(), 1);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with('\u{feff}'));
        assert!(text.contains("\"text: =1+1,\"\"quoted\"\"\n中文\""));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{symlink, PermissionsExt};
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            let link = dir.path().join("link.csv");
            symlink(&path, &link).unwrap();
            assert!(write(&store, &link, &scope, Format::Csv).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        }
    }
}
