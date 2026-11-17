use super::{picker_error, Guard};
use objc2::{rc::Retained, MainThreadMarker};
use objc2_app_kit::{
    NSModalResponseCancel, NSModalResponseOK, NSOpenPanel, NSRunningApplication, NSWindow,
};
use objc2_foundation::{NSArray, NSBundle, NSString, NSURL};
use speakmore_core::{app_rules::macos_identity, error::Result};
use std::cell::RefCell;

fn text(value: &NSString) -> Option<String> {
    if value.length() == 0 || value.length() > 255 {
        return None;
    }
    let value = value.to_string();
    (!value.chars().any(char::is_control) && value.len() <= 512).then_some(value)
}
pub(crate) fn identity(app: &NSRunningApplication) -> Option<String> {
    app.bundleIdentifier()
        .as_deref()
        .and_then(text)
        .and_then(|v| macos_identity(&v).ok())
}
pub(super) fn metadata(url: &NSURL) -> Result<(String, String)> {
    if !url.isFileURL()
        || url
            .pathExtension()
            .is_none_or(|v| !v.to_string().eq_ignore_ascii_case("app"))
    {
        return Err(picker_error());
    }
    let bundle = NSBundle::bundleWithURL(url).ok_or_else(picker_error)?;
    let info = |key| {
        bundle
            .objectForInfoDictionaryKey(&NSString::from_str(key))
            .and_then(|value| value.downcast_ref::<NSString>().and_then(text))
    };
    if info("CFBundlePackageType").as_deref() != Some("APPL") {
        return Err(picker_error());
    }
    let id = bundle
        .bundleIdentifier()
        .as_deref()
        .and_then(text)
        .ok_or_else(picker_error)?;
    let name = info("CFBundleDisplayName")
        .or_else(|| info("CFBundleName"))
        .or_else(|| {
            url.URLByDeletingPathExtension()
                .and_then(|u| u.lastPathComponent())
                .as_deref()
                .and_then(text)
        })
        .ok_or_else(picker_error)?;
    Ok((macos_identity(&id)?, name))
}
pub(super) async fn choose(
    window: &tauri::WebviewWindow,
    guard: Guard,
) -> Result<Option<(String, String)>> {
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
                panel.setTreatsFilePackagesAsDirectories(false);
                panel.setCanCreateDirectories(false);
                #[allow(deprecated)]
                panel.setAllowedFileTypes(Some(&NSArray::from_slice(&[&*NSString::from_str(
                    "app",
                )])));
                panel.setAllowsOtherFileTypes(false);
                panel.setDirectoryURL(Some(&NSURL::fileURLWithPath(&NSString::from_str(
                    "/Applications",
                ))));
                let completion_panel = panel.clone();
                let tx = RefCell::new(Some(tx));
                let callback = block2::RcBlock::new(move |response| {
                    let _keep_picker_reserved = &guard;
                    completion_panel.orderOut(None);
                    let result = if response == NSModalResponseCancel {
                        Ok(None)
                    } else if response == NSModalResponseOK {
                        completion_panel
                            .URL()
                            .ok_or_else(picker_error)
                            .and_then(|u| metadata(&u))
                            .map(Some)
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
    fn reads_only_bounded_application_metadata_without_loading_executable_code() {
        let temp = tempfile::tempdir().unwrap();
        for (name, identifier, kind, valid) in [
            ("Valid.app", "org.Test.Native", "APPL", true),
            ("Upper.APP", "org.Test.Native", "APPL", true),
            ("Bad.app", "/private/path", "APPL", false),
            ("Plugin.app", "org.test.plugin", "BNDL", false),
            ("Document.txt", "org.test.doc", "APPL", false),
        ] {
            let dir = temp.path().join(name);
            std::fs::create_dir_all(dir.join("Contents")).unwrap();
            std::fs::write(dir.join("Contents/Info.plist"),format!(r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>{identifier}</string><key>CFBundlePackageType</key><string>{kind}</string><key>CFBundleDisplayName</key><string>笔记 Native</string></dict></plist>"#)).unwrap();
            let url = NSURL::fileURLWithPath(&NSString::from_str(dir.to_str().unwrap()));
            let result = metadata(&url);
            assert_eq!(result.is_ok(), valid, "{name}");
            if valid {
                assert_eq!(
                    result.unwrap(),
                    ("org.test.native".into(), "笔记 Native".into())
                );
            }
        }
    }
}
