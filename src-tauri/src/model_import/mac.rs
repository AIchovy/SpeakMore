use super::picker_error;
use objc2::{rc::Retained, MainThreadMarker};
use objc2_app_kit::{NSModalResponseCancel, NSModalResponseOK, NSOpenPanel, NSWindow};
use objc2_foundation::{NSArray, NSString, NSURL};
use speakmore_core::error::Result;
use std::{cell::RefCell, path::PathBuf, sync::Arc};

fn paths(urls: &NSArray<NSURL>, expected: usize) -> Result<Vec<PathBuf>> {
    if expected == 0 || expected > 16 || urls.len() != expected {
        return Err(crate::model_error(
            speakmore_models::ModelError::ImportSelection,
        ));
    }
    urls.iter()
        .map(|url| {
            if !url.isFileURL() {
                return Err(picker_error());
            }
            let value = url.path().ok_or_else(picker_error)?;
            if value.length() == 0 || value.length() > 4096 {
                return Err(picker_error());
            }
            let path = PathBuf::from(value.to_string());
            if !path.is_absolute() {
                return Err(picker_error());
            }
            Ok(path)
        })
        .collect()
}

pub(super) async fn choose(
    window: &tauri::WebviewWindow,
    expected: usize,
    guard: Arc<crate::applications::Guard>,
    english: bool,
) -> Result<Option<Vec<PathBuf>>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let host = window.clone();
    window
        .run_on_main_thread(move || {
            let result =
                (|| -> Result<()> {
                    let mtm = MainThreadMarker::new().ok_or_else(picker_error)?;
                    let host = unsafe {
                        Retained::retain(
                            host.ns_window()
                                .map_err(|_| picker_error())?
                                .cast::<NSWindow>(),
                        )
                    }
                    .ok_or_else(picker_error)?;
                    let panel = NSOpenPanel::openPanel(mtm);
                    panel.setCanChooseFiles(true);
                    panel.setCanChooseDirectories(false);
                    panel.setAllowsMultipleSelection(true);
                    panel.setTreatsFilePackagesAsDirectories(false);
                    panel.setResolvesAliases(false);
                    panel.setCanCreateDirectories(false);
                    panel.setMessage(Some(&NSString::from_str(if english {
                "Select every file in the import list. Your original files will stay in place."
            } else { "请选择导入列表中的全部模型文件。原文件会保留在原处。" })));
                    panel.setPrompt(Some(&NSString::from_str(if english {
                        "Import"
                    } else {
                        "导入"
                    })));
                    let completion_panel = panel.clone();
                    let tx = RefCell::new(Some(tx));
                    let callback = block2::RcBlock::new(move |response| {
                        let _keep_picker_reserved = &guard;
                        completion_panel.orderOut(None);
                        let result = if response == NSModalResponseCancel {
                            Ok(None)
                        } else if response == NSModalResponseOK {
                            paths(&completion_panel.URLs(), expected).map(Some)
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
            let _ = result;
        })
        .map_err(|_| picker_error())?;
    rx.await.map_err(|_| picker_error())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn choices_require_a_bounded_complete_set_of_local_file_urls() {
        let file = NSURL::fileURLWithPath(&NSString::from_str("/tmp/语音模型/model.bin"));
        let urls = NSArray::from_slice(&[&*file]);
        assert_eq!(
            paths(&urls, 1).unwrap(),
            [PathBuf::from("/tmp/语音模型/model.bin")]
        );
        assert!(paths(&urls, 0).is_err());
        assert!(paths(&urls, 2).is_err());
        assert!(paths(&urls, 17).is_err());
        let remote =
            NSURL::URLWithString(&NSString::from_str("https://example.invalid/model.bin")).unwrap();
        assert!(paths(&NSArray::from_slice(&[&*remote]), 1).is_err());
    }
}
