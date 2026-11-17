//! Read-only TCC status checks never open a stream or prompt. The only prompt
//! entry point is a main-window command initiated by the user.
use serde::Serialize;
use speakmore_core::error::{AppError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Microphone {
    NotDetermined,
    Restricted,
    Denied,
    Authorized,
    Unavailable,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub microphone: Microphone,
    pub accessibility: bool,
    pub requesting_microphone: bool,
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use block2::RcBlock;
    use objc2::{class, msg_send, runtime::Bool, MainThreadMarker};
    use objc2_foundation::{NSString, NSURL};
    use std::sync::atomic::{AtomicBool, Ordering};

    #[link(name = "AVFoundation", kind = "framework")]
    unsafe extern "C" {
        static AVMediaTypeAudio: &'static NSString;
    }
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        // AXUIElement.h uses C Boolean (UInt8), not Objective-C BOOL.
        fn AXIsProcessTrusted() -> u8;
        fn AXIsProcessTrustedWithOptions(
            options: core_foundation::dictionary::CFDictionaryRef,
        ) -> u8;
        static kAXTrustedCheckOptionPrompt: core_foundation::string::CFStringRef;
    }
    static REQUESTING: AtomicBool = AtomicBool::new(false);

    pub(super) fn accessibility(prompt: bool) -> bool {
        use core_foundation::{
            base::TCFType, boolean::CFBoolean, dictionary::CFDictionary, string::CFString,
        };
        // Called on the main thread by an explicit UI action. A prompt never
        // grants access; its return value remains the actual current trust state.
        let key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
        let options = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::from(prompt))]);
        unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) != 0 }
    }

    pub(super) fn microphone() -> Microphone {
        // SDK AVCaptureDevice.h: NS_ENUM(NSInteger, AVAuthorizationStatus).
        let status: isize = unsafe {
            msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: AVMediaTypeAudio]
        };
        match status {
            0 => Microphone::NotDetermined,
            1 => Microphone::Restricted,
            2 => Microphone::Denied,
            3 => Microphone::Authorized,
            _ => Microphone::Unavailable,
        }
    }
    pub(super) fn read() -> Status {
        Status {
            microphone: microphone(),
            accessibility: unsafe { AXIsProcessTrusted() != 0 },
            requesting_microphone: REQUESTING.load(Ordering::Acquire),
        }
    }
    pub(super) fn request(_mtm: MainThreadMarker) -> Result<()> {
        match microphone() {
            Microphone::Authorized => return Ok(()),
            Microphone::NotDetermined => {}
            _ => {
                return Err(AppError::new(
                    "MicrophonePermissionDenied",
                    "请在系统设置中检查麦克风权限。",
                    true,
                ))
            }
        }
        if REQUESTING.swap(true, Ordering::AcqRel) {
            return Err(AppError::busy());
        }
        // AVFoundation copies the block and invokes it on an arbitrary queue.
        // It only releases admission; the actor reads the actual OS status.
        let completion = RcBlock::new(|_: Bool| REQUESTING.store(false, Ordering::Release));
        unsafe {
            let _: () = msg_send![class!(AVCaptureDevice), requestAccessForMediaType: AVMediaTypeAudio,
                completionHandler: &*completion];
        }
        Ok(())
    }
    pub(super) fn open_settings(_mtm: MainThreadMarker) -> Result<()> {
        let url = NSURL::URLWithString(&NSString::from_str(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
        ))
        .ok_or_else(unavailable)?;
        if objc2_app_kit::NSWorkspace::sharedWorkspace().openURL(&url) {
            Ok(())
        } else {
            Err(AppError::new(
                "SystemSettingsUnavailable",
                "请手动打开系统设置 → 隐私与安全性 → 麦克风。",
                true,
            ))
        }
    }
}
pub fn read() -> Status {
    #[cfg(target_os = "macos")]
    {
        mac::read()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Status {
            microphone: Microphone::Unavailable,
            accessibility: false,
            requesting_microphone: false,
        }
    }
}

pub async fn microphone_action(window: &tauri::WebviewWindow, request: bool) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let (reply, response) = tokio::sync::oneshot::channel();
        window
            .run_on_main_thread(move || {
                let result = objc2::MainThreadMarker::new()
                    .ok_or_else(unavailable)
                    .and_then(|mtm| {
                        if request {
                            mac::request(mtm)
                        } else {
                            mac::open_settings(mtm)
                        }
                    });
                let _ = reply.send(result);
            })
            .map_err(|_| unavailable())?;
        response.await.map_err(|_| unavailable())?
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, request);
        Err(unavailable())
    }
}
pub async fn accessibility_settings(window: &tauri::WebviewWindow) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let (reply, response) = tokio::sync::oneshot::channel();
        window.run_on_main_thread(move || {
            mac::accessibility(true);
            let result = objc2_foundation::NSURL::URLWithString(&objc2_foundation::NSString::from_str(
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"))
                .filter(|url| objc2_app_kit::NSWorkspace::sharedWorkspace().openURL(url))
                .map(|_| ()).ok_or_else(|| AppError::new("SystemSettingsUnavailable",
                    "请手动打开系统设置 → 隐私与安全性 → 辅助功能。", true));
            let _ = reply.send(result);
        }).map_err(|_| unavailable())?;
        response.await.map_err(|_| unavailable())?
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        Err(unavailable())
    }
}
pub async fn check_accessibility(window: &tauri::WebviewWindow) -> Result<bool> {
    #[cfg(target_os = "macos")]
    {
        let (reply, response) = tokio::sync::oneshot::channel();
        window
            .run_on_main_thread(move || {
                let _ = reply.send(mac::accessibility(false));
            })
            .map_err(|_| unavailable())?;
        response.await.map_err(|_| unavailable())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        Err(unavailable())
    }
}
fn unavailable() -> AppError {
    AppError::new(
        "PermissionServiceUnavailable",
        "无法检查系统权限，请重启应用后重试。",
        true,
    )
}
