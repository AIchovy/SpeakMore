use speakmore_core::{
    error::{AppError, Result},
    storage::Store,
};
use std::path::PathBuf;
use tauri::Manager;

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    pub error: Option<AppError>,
    pub cleanup_pending: bool,
    pub can_reveal: bool,
}

pub(crate) struct Recovery {
    directory: Option<PathBuf>,
    pub status: Status,
}

pub(crate) fn open(app: &tauri::App) -> (Result<Store>, Recovery) {
    let directory = app
        .path()
        .app_local_data_dir()
        .ok()
        .map(|root| root.join("data"));
    open_directory(directory)
}

fn open_directory(directory: Option<PathBuf>) -> (Result<Store>, Recovery) {
    let store = (|| -> Result<Store> {
        let directory = directory.as_ref().ok_or_else(unavailable)?;
        match std::fs::symlink_metadata(directory) {
            Ok(meta) if meta.is_dir() => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir_all(directory).map_err(|_| unavailable())?;
            }
            _ => return Err(unavailable()),
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| unavailable())?;
        }
        let mut store = Store::open_with_initial_locale(
            &directory.join("speakmore.sqlite3"),
            crate::native_locale::system_locale().code(),
        )?;
        store.cleanup(crate::now())?;
        Ok(store)
    })();
    let recovery = Recovery {
        status: Status {
            error: store.as_ref().err().cloned(),
            cleanup_pending: store
                .as_ref()
                .is_ok_and(|store| store.migration_backup_retained()),
            can_reveal: directory
                .as_ref()
                .is_some_and(|dir| std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir())),
        },
        directory,
    };
    (store, recovery)
}

fn unavailable() -> AppError {
    AppError::new(
        "StorageUnavailable",
        "无法打开本地数据目录，请检查磁盘空间和权限。",
        true,
    )
}

impl Recovery {
    pub async fn reveal(&self, app: &tauri::AppHandle) -> Result<()> {
        if self.status.error.is_none() && !self.status.cleanup_pending {
            return Err(AppError::new(
                "RecoveryUnavailable",
                "当前没有需要处理的存储启动问题。",
                false,
            ));
        }
        let directory = self.directory.clone().ok_or_else(unavailable)?;
        if !std::fs::symlink_metadata(&directory).is_ok_and(|m| m.is_dir()) {
            return Err(unavailable());
        }
        #[cfg(target_os = "macos")]
        {
            let (tx, rx) = tokio::sync::oneshot::channel();
            app.run_on_main_thread(move || {
                use objc2_app_kit::NSWorkspace;
                use objc2_foundation::{NSArray, NSString, NSURL};
                let url = NSURL::fileURLWithPath(&NSString::from_str(&directory.to_string_lossy()));
                NSWorkspace::sharedWorkspace()
                    .activateFileViewerSelectingURLs(&NSArray::from_slice(&[&*url]));
                let _ = tx.send(());
            })
            .map_err(|_| unavailable())?;
            rx.await.map_err(|_| unavailable())?;
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (app, directory);
            Err(AppError::new(
                "PlatformUnsupported",
                "当前恢复文件入口仅支持 macOS。",
                false,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_startup_has_native_recovery_state_without_exposing_paths() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("data");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(
            directory.join("speakmore.sqlite3"),
            b"preserve corrupt fixture",
        )
        .unwrap();
        let (store, recovery) = open_directory(Some(directory.clone()));
        assert!(store.is_err());
        assert_eq!(
            recovery.status.error.as_ref().unwrap().code,
            "DatabaseCorrupt"
        );
        assert!(recovery.status.can_reveal);
        assert!(!recovery.status.cleanup_pending);
        assert!(!serde_json::to_string(&recovery.status)
            .unwrap()
            .contains(&root.path().to_string_lossy().to_string()));
        assert_eq!(
            std::fs::read(directory.join("speakmore.sqlite3")).unwrap(),
            b"preserve corrupt fixture"
        );
        let (store, recovery) = open_directory(None);
        assert!(store.is_err());
        assert!(!recovery.status.can_reveal);
    }
}
