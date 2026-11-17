//! Paste eligibility uses available metadata, never application allowlists or
//! AX setter support. An editor may accept Cmd+V without exposing text to AX.
use super::{name_ptr, CFGetTypeID, Owned, ProbeResult, Ptr, Reason};
use objc2_core_foundation::CFString;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn IsSecureEventInputEnabled() -> u8;
}
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGSessionCopyCurrentDictionary() -> Ptr;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDictionaryGetTypeID() -> usize;
    fn CFDictionaryGetValue(dictionary: Ptr, key: Ptr) -> Ptr;
    fn CFNumberGetTypeID() -> usize;
    fn CFNumberGetValue(number: Ptr, kind: isize, out: *mut std::ffi::c_void) -> u8;
}

/// Secure Event Input (a focused password field, Secure Keyboard Entry) means
/// keystrokes must stay private, whatever AX reports. The flag is global and
/// can be left on by another process (loginwindow after unlock, a password
/// manager, a terminal): only the target owning it makes the target sensitive.
pub(super) fn secure_input(pid: i32) -> ProbeResult<()> {
    if unsafe { IsSecureEventInputEnabled() } != 0 && secure_input_owner() == Some(pid) {
        return Err(Reason::SensitiveTarget);
    }
    Ok(())
}

fn secure_input_owner() -> Option<i32> {
    // Copy rule: `Owned` releases the session dictionary.
    let session = unsafe { Owned::take(CGSessionCopyCurrentDictionary()) }
        .and_then(|session| session.typed(unsafe { CFDictionaryGetTypeID() }))
        .ok()?;
    let key = CFString::from_str("kCGSSessionSecureInputPID");
    // Get rule: the number is borrowed from the dictionary, never released.
    let value = unsafe { CFDictionaryGetValue(session.ptr(), name_ptr(&key)) };
    if value.is_null() || unsafe { CFGetTypeID(value) != CFNumberGetTypeID() } {
        return None;
    }
    let mut pid = 0_i32;
    // SDK kCFNumberSInt32Type.
    (unsafe { CFNumberGetValue(value, 3, (&mut pid as *mut i32).cast()) } != 0).then_some(pid)
}

pub(super) fn text_role(role: Option<&str>) -> bool {
    matches!(role, Some("AXTextArea" | "AXTextField" | "AXComboBox"))
}

pub(super) fn allows_paste(
    role: Option<&str>,
    subrole: Option<&str>,
    enabled: Option<bool>,
    editable: Option<bool>,
) -> ProbeResult<()> {
    if role == Some("AXSecureTextField") || subrole == Some("AXSecureTextField") {
        return Err(Reason::SensitiveTarget);
    }
    if enabled == Some(false) || editable == Some(false) || role == Some("AXStaticText") {
        return Err(Reason::ReadOnlyTarget);
    }
    // Positively identified non-text controls do not get a paste. Containers,
    // custom/unknown roles and missing roles can host a keyboard text focus.
    if matches!(
        role,
        Some(
            "AXButton"
                | "AXCheckBox"
                | "AXRadioButton"
                | "AXPopUpButton"
                | "AXMenuButton"
                | "AXMenuItem"
                | "AXMenu"
                | "AXMenuBar"
                | "AXSlider"
                | "AXScrollBar"
                | "AXLink"
                | "AXImage"
                | "AXToolbar"
        )
    ) {
        return Err(Reason::UnsupportedTarget);
    }
    Ok(())
}

pub(super) fn unchanged_input(
    clicks_before: [u32; 3],
    clicks_after: [u32; 3],
    keys_before: (u32, u32),
    keys_after: (u32, u32),
) -> ProbeResult<()> {
    // OS counters wrap. Only our own dictation shortcut may add key-downs;
    // Fn contributes flags events, not key-downs. No key content is collected.
    if clicks_before != clicks_after
        || keys_after.0.wrapping_sub(keys_before.0) != keys_after.1.wrapping_sub(keys_before.1)
    {
        return Err(Reason::TargetChanged);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secure_input_left_on_by_another_process_never_blocks_this_target() {
        // This test process never enables Secure Event Input itself.
        assert_eq!(secure_input(std::process::id() as i32), Ok(()));
        let _ = secure_input_owner();
    }

    #[test]
    fn text_and_custom_editors_need_neither_app_allowlists_nor_ax_text_setters() {
        for role in [
            None,
            Some("AXTextArea"),
            Some("AXTextField"),
            Some("AXComboBox"),
            Some("AXGroup"),
            Some("AXWebArea"),
            Some("AXUnknown"),
            Some("AXApplication"),
            Some("AXWindow"),
            Some("CustomEditor"),
        ] {
            assert_eq!(allows_paste(role, None, None, None), Ok(()));
            assert_eq!(allows_paste(role, None, Some(true), None), Ok(()));
        }
    }

    #[test]
    fn missing_metadata_never_overrides_sensitive_or_explicitly_read_only_controls() {
        for role in [None, Some("AXTextField"), Some("AXGroup")] {
            assert_eq!(
                allows_paste(role, Some("AXSecureTextField"), None, None),
                Err(Reason::SensitiveTarget)
            );
            assert_eq!(
                allows_paste(role, None, Some(false), None),
                Err(Reason::ReadOnlyTarget)
            );
            assert_eq!(
                allows_paste(role, None, None, Some(false)),
                Err(Reason::ReadOnlyTarget)
            );
        }
        assert_eq!(
            allows_paste(Some("AXSecureTextField"), None, Some(true), Some(true)),
            Err(Reason::SensitiveTarget)
        );
        assert_eq!(
            allows_paste(Some("AXStaticText"), None, None, None),
            Err(Reason::ReadOnlyTarget)
        );
        for role in ["AXButton", "AXLink", "AXMenuItem", "AXCheckBox"] {
            assert_eq!(
                allows_paste(Some(role), None, None, None),
                Err(Reason::UnsupportedTarget)
            );
        }
    }

    #[test]
    fn all_targets_cancel_on_mouse_or_keyboard_navigation_but_allow_stop_shortcuts() {
        let clicks = [2, 3, 4];
        for keys in [
            ((42, 8), (42, 8)),
            ((42, 8), (43, 9)),
            ((u32::MAX, 8), (0, 9)),
        ] {
            assert_eq!(unchanged_input(clicks, clicks, keys.0, keys.1), Ok(()));
        }
        for next in [[3, 3, 4], [2, 4, 4], [2, 3, 5]] {
            assert_eq!(
                unchanged_input(clicks, next, (42, 8), (42, 8)),
                Err(Reason::TargetChanged)
            );
        }
        assert_eq!(
            unchanged_input(clicks, clicks, (42, 8), (43, 8)),
            Err(Reason::TargetChanged)
        );
        assert_eq!(
            unchanged_input(clicks, clicks, (42, 8), (44, 9)),
            Err(Reason::TargetChanged)
        );
    }
}
