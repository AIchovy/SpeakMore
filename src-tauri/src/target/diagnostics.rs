//! Fixed operation names and API error codes only; never target values or titles.
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
static LAST: AtomicUsize = AtomicUsize::new(0);
static CODE: AtomicI32 = AtomicI32::new(0);
const OPERATIONS: &[&str] = &[
    "Preparing",
    "Accessibility",
    "CodeIdentity",
    "AXObserverCreate",
    "TextEditLayout",
    "AXFocusedApplication",
    "FrontmostApplication",
    "AXCreateApplication",
    "AXFocusedUIElement",
    "AXFocusedWindow",
    "AXRole",
    "AXSubrole",
    "AXParent",
    "AXWindow",
    "AXEnabled",
    "AXIsEditable",
    "InputActivity",
    "SecureEventInput",
    "AXChildren",
    "AXValue",
    "AXSelectedText",
    "AXTitle",
    "AXTitleChanged",
    "AXSelectedTextRange",
    "AXNumberOfCharacters",
    "AXStringForRange",
    "AXFocusedUIElementChanged",
    "AXFocusedWindowChanged",
    "AXApplicationDeactivated",
    "AXSelectedTextChanged",
    "AXValueChanged",
    "AXUIElementDestroyed",
    "Other native check",
];
thread_local! {
    static QUIET: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
/// Reads on other threads (writing context) must not overwrite the probe
/// thread's last operation and error code.
pub(super) struct Quiet(());
pub(super) fn quiet() -> Quiet {
    QUIET.set(true);
    Quiet(())
}
impl Drop for Quiet {
    fn drop(&mut self) {
        QUIET.set(false);
    }
}
pub(super) fn note(operation: &str) {
    if QUIET.get() {
        return;
    }
    CODE.store(0, Ordering::Release);
    LAST.store(
        OPERATIONS
            .iter()
            .position(|value| *value == operation)
            .unwrap_or(OPERATIONS.len() - 1),
        Ordering::Release,
    );
}
pub(crate) fn last() -> &'static str {
    OPERATIONS[LAST.load(Ordering::Acquire)]
}
pub(super) fn status(code: i32) {
    if !QUIET.get() {
        CODE.store(code, Ordering::Release);
    }
}
pub(crate) fn error_code() -> Option<i32> {
    let code = CODE.load(Ordering::Acquire);
    (code != 0).then_some(code)
}

// One overwrite-only report in the application's cache. No transcript, audio,
// app identity, field content, selection, window title or clipboard is recorded.
static REPORT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
pub(crate) fn configure(cache: std::path::PathBuf) {
    let _ = REPORT.set(cache.join("last-input-failure.json"));
}
pub(crate) fn record_failure(phase: &'static str, reason: &'static str) {
    let Some(path) = REPORT.get() else {
        return;
    };
    let report = serde_json::json!({
        "version": 1,
        "at": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
        "phase": phase,
        "operation": last(),
        "nativeError": error_code(),
        "reason": reason,
    });
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let temporary = path.with_extension("tmp");
    if let Ok(bytes) = serde_json::to_vec_pretty(&report) {
        if std::fs::write(&temporary, bytes).is_ok() {
            let _ = std::fs::rename(temporary, path);
        }
    }
}
