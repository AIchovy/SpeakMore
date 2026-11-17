//! SDK AXUIElement.h / AXValue.h adapter. All CF ownership stays on one thread.
mod field;
mod identity;

use super::{Driver, ProbeResult, Watch};
use crate::session_runtime::NativeInput;
use objc2::rc::autoreleasepool;
use objc2_core_foundation::CFString;
use speakmore_core::delivery::ManualReason as Reason;
use std::{
    cell::Cell,
    ffi::c_void,
    ptr::NonNull,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};

type Ptr = *const c_void;
fn note_step(operation: &str) {
    super::diagnostics::note(operation);
}
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct Range {
    location: isize,
    length: isize,
}
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXUIElementCreateApplication(pid: i32) -> Ptr;
    fn AXUIElementGetTypeID() -> usize;
    fn AXUIElementGetPid(element: Ptr, pid: *mut i32) -> i32;
    fn AXUIElementSetMessagingTimeout(element: Ptr, seconds: f32) -> i32;
    fn AXUIElementCopyAttributeValue(element: Ptr, name: Ptr, out: *mut Ptr) -> i32;
    fn AXUIElementSetAttributeValue(element: Ptr, name: Ptr, value: Ptr) -> i32;
    fn CGEventSourceCounterForEventType(state: i32, event_type: u32) -> u32;
    fn AXValueGetTypeID() -> usize;
    #[cfg(test)]
    fn AXValueCreate(kind: u32, value: Ptr) -> Ptr;
    fn AXValueGetType(value: Ptr) -> u32;
    fn AXValueGetValue(value: Ptr, kind: u32, out: *mut c_void) -> u8;
    fn AXObserverCreate(
        pid: i32,
        callback: extern "C" fn(Ptr, Ptr, Ptr, *mut c_void),
        out: *mut Ptr,
    ) -> i32;
    fn AXObserverAddNotification(
        observer: Ptr,
        element: Ptr,
        name: Ptr,
        context: *mut c_void,
    ) -> i32;
    fn AXObserverGetRunLoopSource(observer: Ptr) -> Ptr;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: Ptr);
    fn CFRetain(value: Ptr) -> Ptr;
    fn CFURLGetTypeID() -> usize;
    fn CFURLGetString(value: Ptr) -> Ptr;
    fn CFGetTypeID(value: Ptr) -> usize;
    fn CFEqual(a: Ptr, b: Ptr) -> u8;
    fn CFStringGetTypeID() -> usize;
    fn CFStringGetLength(value: Ptr) -> isize;
    fn CFStringGetCharacters(value: Ptr, range: Range, buffer: *mut u16);
    fn CFBooleanGetTypeID() -> usize;
    fn CFBooleanGetValue(value: Ptr) -> u8;
    fn CFRunLoopGetCurrent() -> Ptr;
    fn CFRunLoopAddSource(run_loop: Ptr, source: Ptr, mode: Ptr);
    fn CFRunLoopRemoveSource(run_loop: Ptr, source: Ptr, mode: Ptr);
    fn CFRunLoopRunInMode(mode: Ptr, seconds: f64, return_after_source: u8) -> i32;
    static kCFRunLoopDefaultMode: Ptr;
    static kCFBooleanTrue: Ptr;
}
struct Owned(NonNull<c_void>);
impl Owned {
    /// Only call with a +1 create/copy result, never a borrowed attribute.
    unsafe fn take(pointer: Ptr) -> ProbeResult<Self> {
        NonNull::new(pointer.cast_mut())
            .map(Self)
            .ok_or(Reason::TargetUnavailable)
    }
    fn ptr(&self) -> Ptr {
        self.0.as_ptr()
    }
    fn typed(self, kind: usize) -> ProbeResult<Self> {
        if unsafe { CFGetTypeID(self.ptr()) } == kind {
            Ok(self)
        } else {
            Err(Reason::TargetUnavailable)
        }
    }
    fn same(&self, other: &Self) -> bool {
        unsafe { CFEqual(self.ptr(), other.ptr()) != 0 }
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CFRelease(self.ptr()) }
    }
}
fn name_ptr(name: &CFString) -> Ptr {
    (name as *const CFString).cast()
}
struct Query {
    deadline: Instant,
    // Per-message AX timeout cap, in seconds.
    message: f32,
    // Set by kAXErrorCannotComplete: the target was busy, not proven changed.
    busy: Cell<bool>,
}
impl Query {
    fn new(deadline: Instant) -> Self {
        Self {
            deadline,
            message: 0.05,
            busy: Cell::new(false),
        }
    }
    /// Heartbeats tolerate a slower single reply; their failures to answer
    /// in time are inconclusive rather than final.
    fn heartbeat(deadline: Instant) -> Self {
        Self {
            message: 0.1,
            ..Self::new(deadline)
        }
    }
    fn time(&self) -> ProbeResult<()> {
        if Instant::now() >= self.deadline {
            Err(Reason::ProbeTimeout)
        } else {
            Ok(())
        }
    }
    fn status(&self, code: i32) -> ProbeResult<()> {
        super::diagnostics::status(code);
        self.time()?;
        if code == 0 {
            Ok(())
        } else if code == -25211 {
            // SDK kAXErrorAPIDisabled can arrive after a trust-status read.
            Err(Reason::AccessibilityRequired)
        } else {
            // SDK kAXErrorCannotComplete: messaging failed or timed out.
            self.busy.set(code == -25204);
            Err(Reason::TargetUnavailable)
        }
    }
    fn timeout(&self, element: &Owned) -> ProbeResult<()> {
        self.time()?;
        let remaining = self
            .deadline
            .saturating_duration_since(Instant::now())
            .as_secs_f32();
        self.status(unsafe {
            AXUIElementSetMessagingTimeout(element.ptr(), remaining.clamp(0.001, self.message))
        })
    }
    // Only an explicitly unsupported/absent attribute may be omitted. Permission,
    // messaging, malformed type and timeout failures must never become fallbacks.
    fn optional(&self, element: &Owned, key: &str) -> ProbeResult<Option<Owned>> {
        note_step(key);
        self.timeout(element)?;
        let name = CFString::from_str(key);
        let mut out = std::ptr::null();
        let status =
            unsafe { AXUIElementCopyAttributeValue(element.ptr(), name_ptr(&name), &mut out) };
        let value = unsafe { Owned::take(out) };
        self.time()?;
        if matches!(status, -25205 | -25212 | -25208) {
            // Preserve the native absence code if this optional value later
            // proves necessary (for example, a missing focused text control).
            super::diagnostics::status(status);
            return Ok(None);
        }
        self.status(status)?;
        value.map(Some)
    }
    fn optional_string(&self, element: &Owned, key: &str) -> ProbeResult<Option<String>> {
        self.optional(element, key)?
            .map(|v| decode_string(&v, 128))
            .transpose()
    }
    /// Electron's documented opt-in builds a full AX tree for the rest of the
    /// process lifetime: offer it once per launch, whatever the focused element
    /// looks like (Chromium may expose a placeholder until it is enabled), and
    /// remember it only once the target answered, so a failed set is retried.
    /// Never mutate a text value or a macOS permission.
    fn expose(&self, app: &Owned, identity: &identity::Identity) -> ProbeResult<()> {
        static OFFERED: Mutex<Vec<(i32, u64)>> = Mutex::new(Vec::new());
        let offered = || OFFERED.lock().unwrap_or_else(|e| e.into_inner());
        if offered().contains(&identity.key()) {
            return Ok(());
        }
        if self.enable_manual_accessibility(app)? {
            let mut offered = offered();
            if !offered.contains(&identity.key()) {
                if offered.len() >= 64 {
                    offered.clear();
                }
                offered.push(identity.key());
            }
        }
        Ok(())
    }
    /// True once the target answered: accepted, or the attribute is unsupported.
    fn enable_manual_accessibility(&self, app: &Owned) -> ProbeResult<bool> {
        // Other frameworks may not expose the attribute.
        self.timeout(app)?;
        let key = CFString::from_str("AXManualAccessibility");
        let status =
            unsafe { AXUIElementSetAttributeValue(app.ptr(), name_ptr(&key), kCFBooleanTrue) };
        // This is an optional framework hint; old Electron builds can report
        // failure even after accepting it. The mandatory AX reads below still
        // validate the actual target and propagate all messaging errors.
        if status == -25211 {
            self.status(status)?;
        }
        self.time()?;
        // SDK kAXErrorAttributeUnsupported: nothing to enable. Busy or failed
        // sets are offered again on the next capture.
        Ok(matches!(status, 0 | -25205))
    }
    fn optional_element(&self, parent: &Owned, key: &str) -> ProbeResult<Option<Owned>> {
        self.optional(parent, key)?
            .map(|v| v.typed(unsafe { AXUIElementGetTypeID() }))
            .transpose()
    }
    fn optional_bool(&self, element: &Owned, key: &str) -> ProbeResult<Option<bool>> {
        self.optional(element, key)?
            .map(|value| {
                let value = value.typed(unsafe { CFBooleanGetTypeID() })?;
                Ok(unsafe { CFBooleanGetValue(value.ptr()) != 0 })
            })
            .transpose()
    }
    fn selected(&self, field: &Owned) -> ProbeResult<Option<Range>> {
        let Some(value) = self.optional(field, "AXSelectedTextRange")? else {
            return Ok(None);
        };
        let value = value.typed(unsafe { AXValueGetTypeID() })?;
        let mut range = Range {
            location: 0,
            length: 0,
        };
        if unsafe {
            AXValueGetType(value.ptr()) != 4
                || AXValueGetValue(value.ptr(), 4, (&mut range as *mut Range).cast()) == 0
        } {
            return Err(Reason::TargetUnavailable);
        }
        if range.location < 0
            || range.length < 0
            || range.location.checked_add(range.length).is_none()
        {
            return Err(Reason::TargetUnavailable);
        }
        Ok(Some(range))
    }
}

fn string_units(value: &Owned, limit: usize) -> ProbeResult<Vec<u16>> {
    if unsafe { CFGetTypeID(value.ptr()) != CFStringGetTypeID() } {
        return Err(Reason::TargetUnavailable);
    }
    let length = unsafe { CFStringGetLength(value.ptr()) };
    let size = usize::try_from(length).map_err(|_| Reason::TargetUnavailable)?;
    if size > limit {
        return Err(Reason::TargetUnavailable);
    }
    let mut units = vec![0; size];
    unsafe {
        CFStringGetCharacters(
            value.ptr(),
            Range {
                location: 0,
                length,
            },
            units.as_mut_ptr(),
        )
    };
    Ok(units)
}
fn decode_string(value: &Owned, limit: usize) -> ProbeResult<String> {
    String::from_utf16(&string_units(value, limit)?).map_err(|_| Reason::TargetUnavailable)
}
struct Observer {
    object: Owned,
    changed: Box<AtomicBool>,
}
extern "C" fn changed(_: Ptr, _: Ptr, _: Ptr, context: *mut c_void) {
    if let Some(flag) = unsafe { context.cast::<AtomicBool>().as_ref() } {
        flag.store(true, Ordering::Release);
    }
}
impl Observer {
    fn new(pid: i32) -> ProbeResult<Self> {
        note_step("AXObserverCreate");
        let mut out = std::ptr::null();
        let status = unsafe { AXObserverCreate(pid, changed, &mut out) };
        super::diagnostics::status(status);
        let object = unsafe { Owned::take(out) }?;
        if status != 0 {
            return Err(Reason::TargetUnavailable);
        }
        unsafe {
            CFRunLoopAddSource(
                CFRunLoopGetCurrent(),
                AXObserverGetRunLoopSource(object.ptr()),
                kCFRunLoopDefaultMode,
            );
        }
        Ok(Self {
            object,
            changed: Box::new(AtomicBool::new(false)),
        })
    }
    fn add(&self, q: &Query, element: &Owned, notifications: &[&str]) -> ProbeResult<()> {
        for key in notifications {
            note_step(key);
            q.timeout(element)?;
            let name = CFString::from_str(key);
            // The observer/source are removed on this same thread before the
            // boxed flag drops. Callback never retains a target or reads text.
            let status = unsafe {
                AXObserverAddNotification(
                    self.object.ptr(),
                    element.ptr(),
                    name_ptr(&name),
                    (&*self.changed as *const AtomicBool).cast_mut().cast(),
                )
            };
            // Unsupported notifications fall back to the existing 100 ms
            // snapshot checks. Registration failures otherwise fail closed.
            if !matches!(status, -25209..=-25207) {
                q.status(status)?;
            }
            q.time()?;
        }
        Ok(())
    }
    fn clean(&self) -> ProbeResult<()> {
        // Process every already queued notification, including move-away/back.
        for _ in 0..64 {
            let handled = unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.0, 1) } == 4;
            if self.changed.load(Ordering::Acquire) {
                return Err(Reason::TargetChanged);
            }
            if !handled {
                return Ok(());
            }
        }
        Err(Reason::TargetUnavailable)
    }
}
impl Drop for Observer {
    fn drop(&mut self) {
        unsafe {
            CFRunLoopRemoveSource(
                CFRunLoopGetCurrent(),
                AXObserverGetRunLoopSource(self.object.ptr()),
                kCFRunLoopDefaultMode,
            );
        }
        // Fields drop in declaration order: release AXObserver before refcon.
    }
}
struct Snapshot {
    app: Owned,
    field: Option<Owned>,
    window: Option<Owned>,
    identity: identity::Identity,
    range: Option<Range>,
}

fn key_presses() -> u32 {
    unsafe { CGEventSourceCounterForEventType(0, 10) }
}
fn mouse_clicks() -> [u32; 3] {
    // Combined session state, left/right/other mouse-down counters. No text,
    // coordinates or event payloads are collected.
    [1, 3, 25].map(|kind| unsafe { CGEventSourceCounterForEventType(0, kind) })
}
fn same_element(a: &Option<Owned>, b: &Option<Owned>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.same(b),
        (None, None) => true,
        _ => false,
    }
}

fn application(q: &Query, pid: i32) -> ProbeResult<Owned> {
    note_step("AXCreateApplication");
    q.time()?;
    if pid <= 0 {
        return Err(Reason::TargetUnavailable);
    }
    let app = unsafe { Owned::take(AXUIElementCreateApplication(pid)) }?
        .typed(unsafe { AXUIElementGetTypeID() })?;
    let mut actual_pid = 0;
    q.status(unsafe { AXUIElementGetPid(app.ptr(), &mut actual_pid) })?;
    if actual_pid != pid {
        return Err(Reason::TargetChanged);
    }
    Ok(app)
}
fn foreground_application(q: &Query) -> ProbeResult<(Owned, identity::Identity)> {
    // The system-wide AXFocusedApplication can return kAXErrorNoValue even
    // when Workspace has an active app. Bind AX directly to that process.
    let identity = identity::frontmost(q)?;
    let app = application(q, identity.pid())?;
    identity::require_frontmost(q, &identity)?;
    Ok((app, identity))
}
fn snapshot(
    q: &Query,
    observer: Option<&Observer>,
    expected: &identity::Identity,
) -> ProbeResult<Snapshot> {
    let (app, identity) = foreground_application(q)?;
    // Reject an identity change before reading any target text or selection.
    if &identity != expected {
        return Err(Reason::TargetChanged);
    }
    // Window/control metadata enhances the guard; absent AX attributes do
    // not prevent ordinary Cmd+V into the keyboard focus of a custom editor.
    let window = q.optional_element(&app, "AXFocusedWindow")?;
    let field = q.optional_element(&app, "AXFocusedUIElement")?;
    let mut range = None;
    if let Some(field) = field.as_ref() {
        let role = q.optional_string(field, "AXRole")?;
        let subrole = q.optional_string(field, "AXSubrole")?;
        // Check sensitive roles before querying selection or editability.
        field::allows_paste(role.as_deref(), subrole.as_deref(), None, None)?;
        let enabled = q.optional_bool(field, "AXEnabled")?;
        let editable = if field::text_role(role.as_deref()) {
            q.optional_bool(field, "AXIsEditable")?
        } else {
            None
        };
        field::allows_paste(role.as_deref(), subrole.as_deref(), enabled, editable)?;
        if let (Some(window), Some(field_window)) =
            (window.as_ref(), q.optional_element(field, "AXWindow")?)
        {
            if !field_window.same(window) {
                return Err(Reason::TargetChanged);
            }
        }
        range = q.selected(field)?;
        if let Some(observer) = observer {
            observer.add(q, field, &["AXSelectedTextChanged", "AXUIElementDestroyed"])?;
        }
    }
    if let (Some(observer), Some(window)) = (observer, window.as_ref()) {
        observer.add(q, window, &["AXUIElementDestroyed"])?;
    }
    identity::require_frontmost(q, &identity)?;
    Ok(Snapshot {
        app,
        field,
        window,
        identity,
        range,
    })
}

/// Read only metadata from the focused window/control and its ancestor chain.
/// Never read AXValue, selected text, page contents, or other tabs.
pub(super) fn writing_context(
    input: &NativeInput,
    generation: u64,
    expected: Option<&str>,
    at: Instant,
) -> ProbeResult<speakmore_core::writing::Context> {
    // Concurrent with the probe thread; must not overwrite its failure report.
    let _quiet = super::diagnostics::quiet();
    autoreleasepool(|_| {
        let q = Query::new(at + std::time::Duration::from_millis(300));
        eligible(input, generation)?;
        q.time()?;
        let (app, identity) = foreground_application(&q)?;
        let running = objc2_app_kit::NSRunningApplication::runningApplicationWithProcessIdentifier(
            identity.pid(),
        )
        .ok_or(Reason::TargetUnavailable)?;
        let bundle =
            crate::applications::mac::identity(&running).ok_or(Reason::TargetUnavailable)?;
        if expected != Some(bundle.as_str()) {
            return Err(Reason::TargetChanged);
        }
        q.expose(&app, &identity)?;
        let window = q.optional_element(&app, "AXFocusedWindow")?;
        let field = q.optional_element(&app, "AXFocusedUIElement")?;
        if let Some(field) = &field {
            if q.optional_string(field, "AXSubrole")?.as_deref() == Some("AXSecureTextField") {
                return Err(Reason::UnsupportedTarget);
            }
        }
        let text = |element: &Owned, key: &str, limit: usize| -> Option<String> {
            q.optional(element, key)
                .ok()
                .flatten()
                .and_then(|v| {
                    if unsafe { CFGetTypeID(v.ptr()) } == unsafe { CFURLGetTypeID() } {
                        let raw = unsafe { CFURLGetString(v.ptr()) };
                        if raw.is_null() {
                            None
                        } else {
                            unsafe { Owned::take(CFRetain(raw)) }
                                .ok()
                                .and_then(|s| decode_string(&s, limit).ok())
                        }
                    } else {
                        decode_string(&v, limit).ok()
                    }
                })
                .filter(|s| !s.trim().is_empty())
        };
        let title = window.as_ref().and_then(|w| text(w, "AXTitle", 512));
        let input_title = field
            .as_ref()
            .and_then(|f| text(f, "AXTitle", 256).or_else(|| text(f, "AXDescription", 256)));
        let mut website_url = window
            .as_ref()
            .and_then(|w| text(w, "AXDocument", 1024))
            .and_then(|v| speakmore_core::writing::public_url(&v));
        let mut node = field;
        for _ in 0..16 {
            if q.time().is_err() || website_url.is_some() {
                break;
            }
            let Some(element) = node.take() else { break };
            website_url =
                text(&element, "AXURL", 1024).and_then(|v| speakmore_core::writing::public_url(&v));
            if website_url.is_none() {
                node = q.optional_element(&element, "AXParent").ok().flatten();
            }
        }
        eligible(input, generation)?;
        identity::require_frontmost(&q, &identity)?;
        let latest = q.optional_element(&app, "AXFocusedWindow")?;
        if !same_element(&window, &latest)
            || latest.as_ref().and_then(|w| text(w, "AXTitle", 512)) != title
        {
            return Err(Reason::TargetChanged);
        }
        Ok(speakmore_core::writing::Context {
            app_identity: Some(bundle),
            app_name: running.localizedName().map(|v| v.to_string()),
            window_title: title,
            input_title,
            website_url,
        })
    })
}

pub(super) struct Probe {
    input: NativeInput,
}
impl Probe {
    /// Runs on the probe thread. Heartbeats compete with local speech
    /// recognition for CPU; a starved probe would look like a slow target.
    pub(super) fn new(input: NativeInput) -> Self {
        unsafe extern "C" {
            fn pthread_set_qos_class_self_np(class: u32, priority: i32) -> i32;
        }
        // SDK qos.h QOS_CLASS_USER_INTERACTIVE.
        unsafe { pthread_set_qos_class_self_np(0x21, 0) };
        Self { input }
    }
}
struct Observed {
    input: NativeInput,
    generation: u64,
    initial: Snapshot,
    observer: Observer,
    keyboard: (u32, u32),
    clicks: [u32; 3],
}
impl Driver for Probe {
    fn capture(&mut self, generation: u64, deadline: Instant) -> ProbeResult<Box<dyn Watch>> {
        autoreleasepool(|_| {
            eligible(&self.input, generation)?;
            // Validate code identity once per dictation; heartbeats reuse it.
            identity::forget();
            let q = Query::new(deadline);
            let keyboard = (key_presses(), self.input.shortcut_key_downs());
            let clicks = mouse_clicks();
            let (app, identity) = foreground_application(&q)?;
            note_step("SecureEventInput");
            field::secure_input(identity.pid())?;
            q.expose(&app, &identity)?;
            let observer = Observer::new(identity.pid())?;
            observer.add(
                &q,
                &app,
                &[
                    "AXFocusedUIElementChanged",
                    "AXFocusedWindowChanged",
                    "AXApplicationDeactivated",
                ],
            )?;
            let initial = snapshot(&q, Some(&observer), &identity)?;
            if !initial.app.same(&app) {
                return Err(Reason::TargetChanged);
            }
            observer.clean()?;
            note_step("InputActivity");
            field::unchanged_input(
                clicks,
                mouse_clicks(),
                keyboard,
                (key_presses(), self.input.shortcut_key_downs()),
            )?;
            eligible(&self.input, generation)?;
            q.time()?;
            Ok(Box::new(Observed {
                input: self.input.clone(),
                generation,
                initial,
                observer,
                keyboard,
                clicks,
            }) as Box<dyn Watch>)
        })
    }
}
fn eligible(input: &NativeInput, generation: u64) -> ProbeResult<()> {
    note_step("Accessibility");
    // Trust only: permissions::read also queries microphone authorization.
    if unsafe { AXIsProcessTrusted() } == 0 {
        return Err(Reason::AccessibilityRequired);
    }
    if input.target_generation() != generation || input.lifecycle().blocked() {
        return Err(Reason::TargetChanged);
    }
    Ok(())
}
impl Watch for Observed {
    fn qualified(&self) -> bool {
        // The original keyboard target is still eligible for one paste. AX
        // metadata is supplemental, not evidence that paste will be accepted.
        true
    }
    fn recheck(&mut self, deadline: Instant) -> ProbeResult<()> {
        self.check(deadline)?;
        note_step("SecureEventInput");
        field::secure_input(self.initial.identity.pid())?;
        // Check at delivery, after the stop shortcut callback has run. Checking
        // during key dispatch would race the callback and reject a valid stop.
        note_step("InputActivity");
        field::unchanged_input(
            self.clicks,
            mouse_clicks(),
            self.keyboard,
            (key_presses(), self.input.shortcut_key_downs()),
        )
    }
    fn check(&mut self, deadline: Instant) -> ProbeResult<()> {
        self.read(&Query::new(deadline))
    }
    fn observe(&mut self, deadline: Instant) -> ProbeResult<bool> {
        let q = Query::heartbeat(deadline);
        match self.read(&q) {
            Ok(()) => Ok(true),
            // A busy target or a starved probe proves nothing about the target.
            Err(Reason::ProbeTimeout) => Ok(false),
            Err(Reason::TargetUnavailable) if q.busy.get() => Ok(false),
            Err(reason) => Err(reason),
        }
    }
    fn sentinel(&self) -> Option<super::Sentinel> {
        let (pid, clicks, keyboard) = (self.initial.identity.pid(), self.clicks, self.keyboard);
        let input = self.input.clone();
        // Runs on the paste writer's thread right before Cmd+V: Workspace and
        // event counters only, never this thread's AX objects.
        Some(Arc::new(move || {
            field::secure_input(pid).is_ok()
                && identity::frontmost_is(pid)
                && field::unchanged_input(
                    clicks,
                    mouse_clicks(),
                    keyboard,
                    (key_presses(), input.shortcut_key_downs()),
                )
                .is_ok()
        }))
    }
}
impl Observed {
    fn read(&mut self, q: &Query) -> ProbeResult<()> {
        autoreleasepool(|_| {
            eligible(&self.input, self.generation)?;
            self.observer.clean()?;
            note_step("InputActivity");
            if self.clicks != mouse_clicks() {
                return Err(Reason::TargetChanged);
            }
            let next = snapshot(q, None, &self.initial.identity)?;
            let old = &self.initial;
            if !old.app.same(&next.app)
                || !same_element(&old.window, &next.window)
                || !same_element(&old.field, &next.field)
                || old.range != next.range
            {
                return Err(Reason::TargetChanged);
            }
            self.observer.clean()?;
            eligible(&self.input, self.generation)?;
            q.time()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn application_target_binds_to_pid_without_system_wide_focus() {
        let q = Query::new(Instant::now() + std::time::Duration::from_secs(1));
        let pid = std::process::id() as i32;
        let first = application(&q, pid).unwrap();
        let second = application(&q, pid).unwrap();
        assert!(first.same(&second));
        assert!(same_element(&None, &None));
        assert!(!same_element(&Some(first), &None));
        assert!(!same_element(&None, &Some(second)));
        assert!(matches!(application(&q, 0), Err(Reason::TargetUnavailable)));
        let expired = Query::new(Instant::now());
        assert!(matches!(
            application(&expired, pid),
            Err(Reason::ProbeTimeout)
        ));
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCharacters(
            allocator: Ptr,
            characters: *const u16,
            count: isize,
        ) -> Ptr;
        fn CFNumberCreate(allocator: Ptr, kind: i32, value: Ptr) -> Ptr;
    }
    #[test]
    fn native_string_types_and_lengths_are_checked_before_copying() {
        let units: Vec<u16> = "本地𠮷abc".encode_utf16().collect();
        let text = unsafe {
            Owned::take(CFStringCreateWithCharacters(
                std::ptr::null(),
                units.as_ptr(),
                units.len() as isize,
            ))
        }
        .unwrap();
        assert_eq!(decode_string(&text, 16).unwrap(), "本地𠮷abc");
        assert!(decode_string(&text, 3).is_err());
        let number = 42_i64;
        let value = unsafe {
            Owned::take(CFNumberCreate(
                std::ptr::null(),
                4,
                (&number as *const i64).cast(),
            ))
        }
        .unwrap();
        assert!(decode_string(&value, 100).is_err());
        let range = Range {
            location: 123,
            length: 5,
        };
        let value =
            unsafe { Owned::take(AXValueCreate(4, (&range as *const Range).cast())) }.unwrap();
        assert_eq!(unsafe { AXValueGetType(value.ptr()) }, 4);
        let mut decoded = Range {
            location: 0,
            length: 0,
        };
        assert_ne!(
            unsafe { AXValueGetValue(value.ptr(), 4, (&mut decoded as *mut Range).cast()) },
            0
        );
        assert!(range == decoded);
    }
}
