use speakmore_core::error::{AppError, Result};
use std::path::PathBuf;
fn picker_error() -> AppError {
    AppError::new("MeetingPicker", "无法打开文件选择器，请重试。", true)
}
#[cfg(target_os = "macos")]
use objc2::{rc::Retained, MainThreadMarker};
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSModalResponseCancel, NSModalResponseOK, NSOpenPanel, NSWindow};
#[cfg(target_os = "macos")]
use objc2_foundation::{NSArray, NSString};
#[cfg(target_os = "macos")]
use std::cell::RefCell;
#[cfg(target_os = "macos")]
pub async fn open(window: &tauri::WebviewWindow) -> Result<Option<PathBuf>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let host = window.clone();
    window
        .run_on_main_thread(move || {
            let result = (|| -> Result<()> {
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
                panel.setAllowsMultipleSelection(false);
                #[allow(deprecated)]
                panel.setAllowedFileTypes(Some(&NSArray::from_slice(&[
                    &*NSString::from_str("wav"),
                    &*NSString::from_str("mp3"),
                    &*NSString::from_str("m4a"),
                    &*NSString::from_str("aac"),
                    &*NSString::from_str("flac"),
                ])));
                panel.setTreatsFilePackagesAsDirectories(false);
                panel.setResolvesAliases(false);
                panel.setCanCreateDirectories(false);
                panel.setMessage(Some(&NSString::from_str(
                    "选择会议录音。源文件会保留在原处。",
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
                            .and_then(|u| u.path())
                            .map(|p| Some(PathBuf::from(p.to_string())))
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
            let _ = result;
        })
        .map_err(|_| picker_error())?;
    rx.await.map_err(|_| picker_error())?
}

#[cfg(target_os = "macos")]
pub async fn save(window: &tauri::WebviewWindow, extension: &str) -> Result<Option<PathBuf>> {
    use objc2::{rc::Retained, MainThreadMarker};
    use objc2_app_kit::{NSModalResponseCancel, NSModalResponseOK, NSSavePanel, NSWindow};
    use objc2_foundation::{NSArray, NSString};
    use std::{cell::RefCell, ffi::CStr, os::unix::ffi::OsStrExt};
    let (tx, rx) = tokio::sync::oneshot::channel();
    let host = window.clone();
    let extension_name = extension.to_string();
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
                let extension = NSString::from_str(extension_name.as_str());
                #[allow(deprecated)]
                panel.setAllowedFileTypes(Some(&NSArray::from_slice(&[&*extension])));
                panel.setAllowsOtherFileTypes(false);
                panel.setCanCreateDirectories(true);
                panel.setShowsTagField(false);
                panel.setNameFieldStringValue(&NSString::from_str(&format!(
                    "SpeakMore-meeting-{}.{}",
                    chrono::Local::now().format("%Y%m%d-%H%M%S"),
                    extension_name.as_str()
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
pub async fn open(_: &tauri::WebviewWindow) -> Result<Option<PathBuf>> {
    Err(picker_error())
}
#[cfg(not(target_os = "macos"))]
pub async fn save(_: &tauri::WebviewWindow, _: &str) -> Result<Option<PathBuf>> {
    Err(picker_error())
}
