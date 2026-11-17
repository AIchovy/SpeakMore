use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, thiserror::Error)]
pub enum ModelError {
    #[error("模型清单的签名无效，请更新应用。")]
    UntrustedCatalog,
    #[error("模型清单内容无效，请更新应用。")]
    InvalidCatalog,
    #[error("此模型不支持当前系统或引擎版本。")]
    UnsupportedModel,
    #[error("当前模型任务尚未结束，请稍后再试。")]
    Busy,
    #[error("模型正在使用，请先切换或卸载模型。")]
    InUse,
    #[error("模型安装已取消。")]
    Cancelled,
    #[error("正在完成安装，已无法取消。")]
    Committing,
    #[error("磁盘空间不足，请释放空间后重试。")]
    InsufficientSpace,
    #[error("网络连接中断或超时，请检查网络后重试。")]
    Network,
    #[error("下载服务暂时不可用，请稍后重试。")]
    Server,
    #[error("下载地址无法提供此模型文件，请更新应用后重试，或从本机导入。")]
    HttpStatus,
    #[error("文件大小不符，已保留之前的模型。")]
    SizeMismatch,
    #[error("文件校验失败，已保留之前的模型。")]
    HashMismatch,
    #[error("无法安全读写模型目录，请检查权限后重试。")]
    Storage,
    #[error("模型文件已就位，但未能登记，请重新检查本地模型。")]
    Registration,
    #[error("模型未安装或文件不完整，请重新下载或导入。")]
    NotInstalled,
    #[error("请选择文件列表中的全部文件，保持原文件名，不要选择文件夹、压缩包或其他文件。")]
    ImportSelection,
}
impl ModelError {
    pub fn retryable(self) -> bool {
        matches!(
            self,
            Self::Network
                | Self::Server
                | Self::Storage
                | Self::Registration
                | Self::InsufficientSpace
        )
    }
    pub(crate) fn automatic_retry(self) -> bool {
        matches!(self, Self::Network | Self::Server)
    }
}
impl From<std::io::Error> for ModelError {
    fn from(error: std::io::Error) -> Self {
        #[cfg(unix)]
        if matches!(error.raw_os_error(), Some(libc::ENOSPC | libc::EDQUOT)) {
            return Self::InsufficientSpace;
        }
        let _ = error;
        Self::Storage
    }
}
pub type Result<T> = std::result::Result<T, ModelError>;
