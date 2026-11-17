//! Consume a bound Fn tap before it also reaches the system input-source
//! shortcut. Ordinary Fn chords pass through; no character data is read.
//! The tap owns a dedicated thread and run loop: every system key event waits
//! on this filter, so main-thread work must never delay typing.
use objc2::MainThreadMarker;
use speakmore_core::error::{AppError, Result};
use std::{
    ffi::c_void,
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::{Duration, Instant},
};

type Ptr = *const c_void;
type Event = *mut c_void;
// Intercept before the login-session layer, where the system may already have
// handled the Globe/Fn input-source gesture. Permission is decided by macOS;
// do not silently fall back to a later tap that cannot provide this behavior.
pub(crate) const KEYBOARD_TAP_LOCATION: u32 = 0; // kCGHIDEventTap
const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapCreate(
        location: u32,
        placement: u32,
        options: u32,
        mask: u64,
        callback: extern "C" fn(Ptr, u32, Event, *mut c_void) -> Event,
        context: *mut c_void,
    ) -> Ptr;
    fn CGEventTapEnable(tap: Ptr, enabled: bool);
    fn CGEventGetFlags(event: Ptr) -> u64;
    fn CGEventGetIntegerValueField(event: Ptr, field: u32) -> i64;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFMachPortCreateRunLoopSource(allocator: Ptr, port: Ptr, order: isize) -> Ptr;
    fn CFMachPortInvalidate(port: Ptr);
    fn CFRunLoopGetCurrent() -> Ptr;
    fn CFRunLoopRun();
    fn CFRunLoopStop(run_loop: Ptr);
    fn CFRunLoopAddSource(run_loop: Ptr, source: Ptr, mode: Ptr);
    fn CFRunLoopRemoveSource(run_loop: Ptr, source: Ptr, mode: Ptr);
    fn CFRetain(value: Ptr) -> Ptr;
    fn CFRelease(value: Ptr);
    static kCFRunLoopCommonModes: Ptr;
}
unsafe extern "C" {
    fn pthread_set_qos_class_self_np(qos: u32, relative_priority: i32) -> i32;
}
#[derive(Default)]
struct Gesture {
    started: Option<Instant>,
    chord: bool,
    epoch: (u64, u64),
}
impl Gesture {
    fn event(
        &mut self,
        flags_changed: bool,
        key: u16,
        function: bool,
        other: bool,
        epoch: (u64, u64),
    ) -> bool {
        if flags_changed && key == 63 {
            if function {
                if self.started.is_none() {
                    self.started = Some(Instant::now());
                    self.chord = other;
                    self.epoch = epoch;
                }
            } else if let Some(started) = self.started.take() {
                return !self.chord
                    && !other
                    && self.epoch == epoch
                    && started.elapsed().as_secs_f32() < 2.0;
            }
        } else if self.started.is_some() {
            // Fn + arrows/function keys/modifiers must keep their normal use.
            self.chord = true;
        }
        false
    }
}
#[derive(Default)]
struct Filter {
    gesture: Gesture,
    consumed: bool,
}
impl Filter {
    fn event(
        &mut self,
        kind: u32,
        key: u16,
        flags: u64,
        epoch: (u64, u64),
        active: bool,
    ) -> (bool, bool) {
        let function = flags & (1 << 23) != 0;
        let other = flags & ((1 << 17) | (1 << 18) | (1 << 19) | (1 << 20)) != 0;
        if kind == 12 && key == 63 {
            if function {
                if self.gesture.started.is_none() {
                    self.consumed = active && !other;
                }
                self.gesture.event(true, key, function, other, epoch);
                return (self.consumed, false);
            }
            let consumed = std::mem::take(&mut self.consumed);
            let tap = self.gesture.event(true, key, false, other, epoch);
            return (consumed, consumed && active && tap);
        }
        self.gesture.event(kind == 12, key, function, other, epoch);
        (false, false)
    }
    /// A disabled tap may have lost the Fn release: forget the press so the
    /// next tap starts fresh. A late release of a consumed press stays
    /// consumed and never replays the interrupted tap.
    fn interrupt(&mut self) {
        self.gesture.started = None;
    }
}
struct Callback {
    tap: Ptr,
    filter: Filter,
    input: crate::session_runtime::NativeInput,
    app: tauri::AppHandle,
    active: Arc<dyn Fn() -> bool + Send + Sync>,
    action: Arc<dyn Fn() + Send + Sync>,
}
extern "C" fn event(_: Ptr, kind: u32, raw: Event, context: *mut c_void) -> Event {
    // Only the tap thread's run loop calls back; it removes and invalidates the
    // port after the loop stops, before the callback state is dropped.
    let Some(state) = (unsafe { context.cast::<Callback>().as_mut() }) else {
        return raw;
    };
    if kind == u32::MAX || kind == u32::MAX - 1 {
        state.filter.interrupt();
        unsafe { CGEventTapEnable(state.tap, true) };
        return raw;
    }
    if raw.is_null() {
        return raw;
    }
    let lifecycle = state.input.lifecycle();
    let epoch = (lifecycle.epoch, state.input.control_revision());
    let active = (state.active)() && !lifecycle.blocked() && !state.input.editing();
    let (consume, tapped) = state.filter.event(
        kind,
        unsafe { CGEventGetIntegerValueField(raw, 9) as u16 },
        unsafe { CGEventGetFlags(raw) },
        epoch,
        active,
    );
    if tapped {
        let action = state.action.clone();
        let active = state.active.clone();
        let input = state.input.clone();
        // Off the main thread this only posts: never block the tap on window
        // work, IPC or a permission query.
        let _ = state.app.run_on_main_thread(move || {
            let lifecycle = input.lifecycle();
            if active()
                && !lifecycle.blocked()
                && !input.editing()
                && (lifecycle.epoch, input.control_revision()) == epoch
            {
                action();
            }
        });
    }
    if consume {
        std::ptr::null_mut()
    } else {
        raw
    }
}
/// One retain on the tap thread's run loop, so another thread can stop it.
struct RunLoop(Ptr);
// SAFETY: CFRunLoopStop and CFRelease are thread-safe on a retained run loop.
unsafe impl Send for RunLoop {}
impl Drop for RunLoop {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}
struct Monitor {
    generation: u64,
    run_loop: RunLoop,
    thread: Option<JoinHandle<()>>,
}
impl Drop for Monitor {
    fn drop(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        // CFRunLoopStop is a no-op until the loop runs; repeat until it exits.
        // The callback never waits on another thread, so this stays bounded.
        while !thread.is_finished() {
            unsafe { CFRunLoopStop(self.run_loop.0) };
            std::thread::sleep(Duration::from_millis(1));
        }
        let _ = thread.join();
    }
}
static MONITOR: Mutex<Option<Monitor>> = Mutex::new(None);
fn unavailable() -> AppError {
    AppError::new(
        "ControlsUnavailable",
        "无法接管 Fn 按键，请确认辅助功能权限并重启 SpeakMore 后重试。",
        true,
    )
}
pub(crate) fn main_thread<T: Send + 'static>(
    app: &tauri::AppHandle,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    if MainThreadMarker::new().is_some() {
        return work();
    }
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let _ = send.send(work());
    })
    .map_err(|_| unavailable())?;
    receive.recv().map_err(|_| unavailable())?
}
fn run(mut callback: Box<Callback>, ready: std::sync::mpsc::SyncSender<Result<RunLoop>>) {
    // Keyboard latency for every application depends on this thread.
    unsafe { pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0) };
    // HID head, active filter (not listen-only). If unavailable, report
    // registration failure; never escalate privileges or edit Fn settings.
    let tap = unsafe {
        CGEventTapCreate(
            KEYBOARD_TAP_LOCATION,
            0,
            0,
            (1 << 10) | (1 << 12),
            event,
            (&mut *callback as *mut Callback).cast(),
        )
    };
    if tap.is_null() {
        let _ = ready.send(Err(unavailable()));
        return;
    }
    callback.tap = tap;
    let source = unsafe { CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0) };
    if source.is_null() {
        unsafe {
            CFMachPortInvalidate(tap);
            CFRelease(tap);
        }
        let _ = ready.send(Err(unavailable()));
        return;
    }
    let run_loop = unsafe { CFRunLoopGetCurrent() };
    unsafe {
        CFRunLoopAddSource(run_loop, source, kCFRunLoopCommonModes);
        CFRetain(run_loop);
    }
    if ready.send(Ok(RunLoop(run_loop))).is_ok() {
        unsafe { CFRunLoopRun() };
    }
    unsafe {
        CFRunLoopRemoveSource(run_loop, source, kCFRunLoopCommonModes);
        CFMachPortInvalidate(tap);
        CFRelease(source);
        CFRelease(tap);
    }
    drop(callback);
}
pub(crate) fn register(
    app: &tauri::AppHandle,
    generation: u64,
    input: crate::session_runtime::NativeInput,
    active: impl Fn() -> bool + Send + Sync + 'static,
    action: impl Fn() + Send + Sync + 'static,
) -> Result<()> {
    if !crate::permissions::read().accessibility {
        return Err(AppError::new(
            "FnPermissionRequired",
            "请在系统设置中允许 SpeakMore 使用辅助功能，然后重启应用，无需重新保存快捷键。",
            true,
        ));
    }
    let mut slot = MONITOR.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_some() {
        return Err(AppError::new(
            "ShortcutConflict",
            "Fn 已分配给另一个动作。",
            true,
        ));
    }
    let app = app.clone();
    let (ready, receive) = std::sync::mpsc::sync_channel(1);
    let thread = std::thread::Builder::new()
        .name("speakmore-fn-tap".into())
        .spawn(move || {
            let callback = Box::new(Callback {
                tap: std::ptr::null(),
                filter: Filter::default(),
                input,
                app,
                active: Arc::new(active),
                action: Arc::new(action),
            });
            run(callback, ready)
        })
        .map_err(|_| unavailable())?;
    match receive.recv().unwrap_or_else(|_| Err(unavailable())) {
        Ok(run_loop) => {
            *slot = Some(Monitor {
                generation,
                run_loop,
                thread: Some(thread),
            });
            Ok(())
        }
        Err(error) => {
            let _ = thread.join();
            Err(error)
        }
    }
}
pub(crate) fn unregister(generation: u64) -> Result<()> {
    let monitor = {
        let mut slot = MONITOR.lock().unwrap_or_else(|e| e.into_inner());
        slot.take_if(|m| m.generation == generation)
    };
    drop(monitor);
    Ok(())
}
pub(crate) fn destroy() {
    let monitor = MONITOR.lock().unwrap_or_else(|e| e.into_inner()).take();
    drop(monitor);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bound_fn_is_consumed_but_other_keys_and_disabled_bindings_pass_through() {
        let mut filter = Filter::default();
        let function = 1 << 23;
        assert_eq!(filter.event(12, 63, function, (0, 0), true), (true, false));
        assert_eq!(filter.event(12, 63, function, (0, 0), true), (true, false));
        assert_eq!(filter.event(12, 63, 0, (0, 0), true), (true, true));
        assert_eq!(filter.event(12, 63, 0, (0, 0), true), (false, false));
        filter.event(12, 63, function, (0, 0), true);
        assert_eq!(
            filter.event(10, 126, function, (0, 0), true),
            (false, false)
        );
        assert_eq!(filter.event(12, 63, 0, (0, 0), true), (true, false));
        assert_eq!(
            filter.event(12, 63, function, (0, 0), false),
            (false, false)
        );
        assert_eq!(filter.event(12, 63, 0, (0, 0), true), (false, false));
        filter.event(12, 63, function, (0, 0), true);
        assert_eq!(filter.event(12, 63, 0, (1, 0), true), (true, false));
        assert_eq!(
            filter.event(12, 63, function | (1 << 20), (1, 0), true),
            (false, false)
        );
        assert_eq!(filter.event(12, 63, 1 << 20, (1, 0), true), (false, false));
    }
    #[test]
    fn tap_disabled_mid_press_never_replays_or_swallows_the_next_tap() {
        let mut filter = Filter::default();
        let function = 1 << 23;
        // The release was lost while the tap was disabled.
        assert_eq!(filter.event(12, 63, function, (0, 0), true), (true, false));
        filter.interrupt();
        assert_eq!(filter.event(12, 63, function, (0, 0), true), (true, false));
        assert_eq!(filter.event(12, 63, 0, (0, 0), true), (true, true));
        // A release delivered after re-enabling stays consumed but inert.
        filter.event(12, 63, function, (0, 0), true);
        filter.interrupt();
        assert_eq!(filter.event(12, 63, 0, (0, 0), true), (true, false));
        assert_eq!(filter.event(12, 63, function, (0, 0), true), (true, false));
        assert_eq!(filter.event(12, 63, 0, (0, 0), true), (true, true));
    }
    #[test]
    fn monitor_drop_stops_and_joins_a_run_loop_even_before_it_runs() {
        unsafe extern "C" {
            fn CFMachPortCreate(
                allocator: Ptr,
                callout: extern "C" fn(Ptr, Ptr, isize, *mut c_void),
                context: *const usize,
                should_free: *mut u8,
            ) -> Ptr;
        }
        extern "C" fn ignore(_: Ptr, _: Ptr, _: isize, _: *mut c_void) {}
        for delay in [0, 20] {
            let (ready, receive) = std::sync::mpsc::sync_channel(1);
            let thread = std::thread::spawn(move || unsafe {
                let context = [0usize; 5];
                let port = CFMachPortCreate(
                    std::ptr::null(),
                    ignore,
                    context.as_ptr(),
                    std::ptr::null_mut(),
                );
                let source = CFMachPortCreateRunLoopSource(std::ptr::null(), port, 0);
                let run_loop = CFRunLoopGetCurrent();
                CFRunLoopAddSource(run_loop, source, kCFRunLoopCommonModes);
                CFRetain(run_loop);
                ready.send(RunLoop(run_loop)).unwrap();
                std::thread::sleep(Duration::from_millis(delay));
                CFRunLoopRun();
                CFRunLoopRemoveSource(run_loop, source, kCFRunLoopCommonModes);
                CFMachPortInvalidate(port);
                CFRelease(source);
                CFRelease(port);
            });
            let run_loop = receive.recv().unwrap();
            if delay == 0 {
                std::thread::sleep(Duration::from_millis(20));
            }
            drop(Monitor {
                generation: 0,
                run_loop,
                thread: Some(thread),
            });
        }
    }
    #[test]
    fn fn_only_taps_ignore_chords_repeats_or_interrupted_presses() {
        let mut g = Gesture::default();
        assert!(!g.event(true, 63, false, false, (0, 0)));
        assert!(!g.event(true, 63, true, false, (0, 0)));
        assert!(!g.event(true, 63, true, false, (0, 0)));
        assert!(g.event(true, 63, false, false, (0, 0)));
        assert!(!g.event(true, 63, false, false, (0, 0)));
        for (flags, key, other) in [(false, 126, false), (false, 122, false), (true, 56, true)] {
            g.event(true, 63, true, false, (0, 0));
            assert!(!g.event(flags, key, true, other, (0, 0)));
            assert!(!g.event(true, 63, false, false, (0, 0)));
        }
        g.event(true, 63, true, false, (0, 0));
        assert!(!g.event(true, 63, false, false, (1, 0)));
        g.event(true, 63, true, false, (1, 0));
        assert!(!g.event(true, 63, false, false, (1, 1)));
        g.event(true, 63, true, true, (1, 0));
        assert!(!g.event(true, 63, false, false, (1, 0)));
    }
}
