use serde::Serialize;

#[derive(Debug, Clone, Serialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{user_message}")]
pub struct AppError {
    pub code: String,
    pub user_message: String,
    pub retryable: bool,
}

impl AppError {
    pub fn new(code: &str, message: &str, retryable: bool) -> Self {
        Self {
            code: code.into(),
            user_message: message.into(),
            retryable,
        }
    }
    pub fn invalid(message: &str) -> Self {
        Self::new("InvalidInput", message, false)
    }
    pub fn busy() -> Self {
        Self::new("Busy", "当前听写尚未结束，请稍后再试。", true)
    }
    pub fn conflict() -> Self {
        Self::new(
            "RevisionConflict",
            "设置已在其他位置改变，请刷新后重试。",
            true,
        )
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(_: rusqlite::Error) -> Self {
        // SQL and file paths may include sensitive text; never return them to a WebView.
        Self::new(
            "StorageUnavailable",
            "无法读写本地数据，请检查磁盘空间后重试。",
            true,
        )
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
