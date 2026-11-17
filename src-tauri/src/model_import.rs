//! Native choices stay inside Rust; the IPC caller only chooses a catalog model ID.
use speakmore_core::error::{AppError, Result};
use speakmore_models::{ModelManager, PreparedImport};

#[cfg(target_os = "macos")]
mod mac;

pub(crate) async fn choose(
    window: &tauri::WebviewWindow,
    manager: ModelManager,
    model_id: String,
    english: bool,
) -> Result<Option<PreparedImport>> {
    #[cfg(target_os = "macos")]
    {
        let snapshot = manager.snapshot();
        if snapshot.busy {
            return Err(AppError::busy());
        }
        let row = snapshot
            .models
            .iter()
            .find(|m| m.model_id == model_id)
            .ok_or_else(|| AppError::invalid("模型不在已发布清单中。"))?;
        if !row.supported {
            return Err(super::model_error(
                speakmore_models::ModelError::UnsupportedModel,
            ));
        }
        if row.state == speakmore_models::InstallState::Installed {
            return Err(super::model_error(speakmore_models::ModelError::InUse));
        }
        let guard = std::sync::Arc::new(crate::applications::Guard::acquire()?);
        let Some(paths) = mac::choose(window, row.artifacts.len(), guard.clone(), english).await?
        else {
            return Ok(None);
        };
        // Keep the single native selection reserved while an external disk opens.
        tokio::task::spawn_blocking(move || {
            let _keep_selection_reserved = guard;
            manager
                .prepare_import(&model_id, paths)
                .map(Some)
                .map_err(super::model_error)
        })
        .await
        .map_err(|_| picker_error())?
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, manager, model_id, english);
        Err(AppError::new(
            "UnsupportedPlatform",
            "本地模型导入当前仅支持 macOS。",
            false,
        ))
    }
}
fn picker_error() -> AppError {
    AppError::new(
        "ModelPickerUnavailable",
        "无法读取所选模型文件，请重新选择。",
        true,
    )
}
