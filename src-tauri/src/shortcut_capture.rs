//! A temporary, active HID-head event tap. Never read characters or log keys.
//! The edit lease and foreground app bound the capture; no WebView keydown fallback.
use crate::session_runtime::NativeInput;
use objc2_app_kit::NSRunningApplication;
use serde::Serialize;
use speakmore_core::error::{AppError, Result};
use std::{
    cell::RefCell,
    collections::HashSet,
    ffi::c_void,
    time::{Duration, Instant},
};
use tauri::Emitter;
use uuid::Uuid;

type Ptr = *const c_void;
type Event = *mut c_void;
const MODIFIERS: u64 = (1 << 17) | (1 << 18) | (1 << 19) | (1 << 20);
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
    fn CGEventSourceFlagsState(state: i32) -> u64;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFMachPortCreateRunLoopSource(allocator: Ptr, port: Ptr, order: isize) -> Ptr;
    fn CFMachPortInvalidate(port: Ptr);
    fn CFRunLoopGetMain() -> Ptr;
    fn CFRunLoopAddSource(run_loop: Ptr, source: Ptr, mode: Ptr);
    fn CFRunLoopRemoveSource(run_loop: Ptr, source: Ptr, mode: Ptr);
    fn CFRelease(value: Ptr);
    static kCFRunLoopCommonModes: Ptr;
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Outcome {
    Captured { accelerator: String },
    Invalid,
    Cancelled,
    Interrupted,
}
#[derive(Default)]
struct Gesture {
    down: HashSet<u16>,
    candidate: Option<Outcome>,
    fn_down: bool,
    fn_chord: bool,
}
impl Gesture {
    fn event(&mut self, kind: u32, key: u16, flags: u64) -> Option<Outcome> {
        match kind {
            10 => {
                if !self.down.insert(key) {
                    return None;
                } // auto-repeat
                if self.fn_down {
                    self.fn_chord = true;
                }
                self.candidate = Some(if self.down.len() > 1 || self.fn_down {
                    Outcome::Invalid
                } else if key == 53 && flags & MODIFIERS == 0 {
                    Outcome::Cancelled
                } else {
                    accelerator(key, flags).map_or(Outcome::Invalid, |accelerator| {
                        Outcome::Captured { accelerator }
                    })
                });
            }
            11 => {
                self.down.remove(&key);
            }
            12 if key == 63 => {
                if flags & (1 << 23) != 0 {
                    self.fn_down = true;
                    self.fn_chord = flags & MODIFIERS != 0 || !self.down.is_empty();
                } else if self.fn_down {
                    self.fn_down = false;
                    self.candidate = Some(if self.fn_chord {
                        Outcome::Invalid
                    } else {
                        Outcome::Captured {
                            accelerator: "Fn".into(),
                        }
                    });
                }
            }
            12 if self.fn_down => self.fn_chord = true,
            _ => {}
        }
        // Consume the complete gesture, including key-up and modifier releases.
        if self.down.is_empty() && !self.fn_down && flags & MODIFIERS == 0 {
            self.candidate.take()
        } else {
            None
        }
    }
}
fn accelerator(code: u16, flags: u64) -> Option<String> {
    let key = match code {
        0 => "KeyA",
        1 => "KeyS",
        2 => "KeyD",
        3 => "KeyF",
        4 => "KeyH",
        5 => "KeyG",
        6 => "KeyZ",
        7 => "KeyX",
        8 => "KeyC",
        9 => "KeyV",
        11 => "KeyB",
        12 => "KeyQ",
        13 => "KeyW",
        14 => "KeyE",
        15 => "KeyR",
        16 => "KeyY",
        17 => "KeyT",
        18 => "Digit1",
        19 => "Digit2",
        20 => "Digit3",
        21 => "Digit4",
        22 => "Digit6",
        23 => "Digit5",
        24 => "Equal",
        25 => "Digit9",
        26 => "Digit7",
        27 => "Minus",
        28 => "Digit8",
        29 => "Digit0",
        30 => "BracketRight",
        31 => "KeyO",
        32 => "KeyU",
        33 => "BracketLeft",
        34 => "KeyI",
        35 => "KeyP",
        36 => "Enter",
        37 => "KeyL",
        38 => "KeyJ",
        39 => "Quote",
        40 => "KeyK",
        41 => "Semicolon",
        42 => "Backslash",
        43 => "Comma",
        44 => "Slash",
        45 => "KeyN",
        46 => "KeyM",
        47 => "Period",
        48 => "Tab",
        49 => "Space",
        50 => "Backquote",
        51 => "Backspace",
        53 => "Escape",
        64 => "F17",
        79 => "F18",
        80 => "F19",
        90 => "F20",
        96 => "F5",
        97 => "F6",
        98 => "F7",
        99 => "F3",
        100 => "F8",
        101 => "F9",
        103 => "F11",
        105 => "F13",
        106 => "F16",
        107 => "F14",
        109 => "F10",
        111 => "F12",
        113 => "F15",
        115 => "Home",
        116 => "PageUp",
        117 => "Delete",
        118 => "F4",
        119 => "End",
        120 => "F2",
        121 => "PageDown",
        122 => "F1",
        123 => "ArrowLeft",
        124 => "ArrowRight",
        125 => "ArrowDown",
        126 => "ArrowUp",
        _ => return None,
    };
    let mut parts: Vec<&str> = [(18, "Control"), (19, "Alt"), (17, "Shift"), (20, "Super")]
        .into_iter()
        .filter_map(|(bit, name)| (flags & (1 << bit) != 0).then_some(name))
        .collect();
    parts.push(key);
    let value = parts.join("+");
    speakmore_core::shortcuts::validate(&value)
        .ok()
        .map(|_| value)
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Message {
    edit_id: Uuid,
    capture_id: Uuid,
    #[serde(flatten)]
    outcome: Outcome,
}
struct Callback {
    tap: Ptr,
    app: tauri::AppHandle,
    input: NativeInput,
    edit_id: Uuid,
    capture_id: Uuid,
    started: Instant,
    gesture: Gesture,
    done: bool,
}
fn foreground() -> bool {
    NSRunningApplication::currentApplication().isActive()
}
impl Callback {
    fn valid(&self) -> bool {
        !self.done
            && self.started.elapsed() < Duration::from_secs(30)
            && self.input.edit_matches(self.edit_id)
            && foreground()
    }
    fn finish(&mut self, outcome: Outcome) {
        if self.done {
            return;
        }
        self.done = true;
        unsafe {
            CGEventTapEnable(self.tap, false);
        }
        let app = self.app.clone();
        let message = Message {
            edit_id: self.edit_id,
            capture_id: self.capture_id,
            outcome,
        };
        // Never perform IPC in the event-tap callback.
        tauri::async_runtime::spawn(async move {
            let _ = app.emit_to("main", "shortcut_capture", message);
        });
    }
}
extern "C" fn event(_: Ptr, kind: u32, raw: Event, context: *mut c_void) -> Event {
    let Some(state) = (unsafe { context.cast::<Callback>().as_mut() }) else {
        return raw;
    };
    if state.done {
        return raw;
    }
    if kind == u32::MAX || kind == u32::MAX - 1 {
        state.finish(Outcome::Interrupted); // no silent re-enable after lost events
        return raw;
    }
    if raw.is_null() {
        return raw;
    }
    if !state.valid() || matches!(kind, 1 | 3 | 25) {
        state.finish(Outcome::Cancelled);
        return raw; // mouse actions and the next app's input are never swallowed
    }
    if matches!(kind, 10..=12) {
        let outcome = state.gesture.event(
            kind,
            unsafe { CGEventGetIntegerValueField(raw, 9) as u16 },
            unsafe { CGEventGetFlags(raw) },
        );
        if let Some(outcome) = outcome {
            state.finish(outcome);
        }
        std::ptr::null_mut() // suppress at OS level, not merely DOM preventDefault
    } else {
        raw
    }
}
struct Monitor {
    tap: Ptr,
    source: Ptr,
    callback: Box<Callback>,
}
impl Drop for Monitor {
    fn drop(&mut self) {
        unsafe {
            CFRunLoopRemoveSource(CFRunLoopGetMain(), self.source, kCFRunLoopCommonModes);
            CFMachPortInvalidate(self.tap);
            CFRelease(self.source);
            CFRelease(self.tap);
        }
    }
}
thread_local! { static MONITOR:RefCell<Option<Monitor>> = const { RefCell::new(None) }; }
fn unavailable() -> AppError {
    AppError::new(
        "ShortcutCaptureUnavailable",
        "无法安全录入快捷键，请检查辅助功能权限后重试。",
        true,
    )
}
pub(crate) fn start(
    app: &tauri::AppHandle,
    input: NativeInput,
    edit_id: Uuid,
    capture_id: Uuid,
) -> Result<()> {
    if !crate::permissions::read().accessibility {
        return Err(unavailable());
    }
    let handle = app.clone();
    let result = crate::fn_shortcut::main_thread(app, move || {
        if !input.edit_matches(edit_id) || !foreground() {
            return Err(AppError::busy());
        }
        // Starting with a held modifier would leave the other apps half a gesture.
        if unsafe { CGEventSourceFlagsState(0) } & (MODIFIERS | (1 << 23)) != 0 {
            return Err(AppError::invalid("请先松开所有按键，再点击录入。"));
        }
        MONITOR.with(|slot| {
            slot.borrow_mut().take();
            let mut callback = Box::new(Callback {
                tap: std::ptr::null(),
                app: handle,
                input,
                edit_id,
                capture_id,
                started: Instant::now(),
                gesture: Gesture::default(),
                done: false,
            });
            let tap = unsafe {
                CGEventTapCreate(
                    crate::fn_shortcut::KEYBOARD_TAP_LOCATION,
                    0,
                    0,
                    (1 << 10) | (1 << 11) | (1 << 12) | (1 << 1) | (1 << 3) | (1 << 25),
                    event,
                    (&mut *callback as *mut Callback).cast(),
                )
            };
            if tap.is_null() {
                return Err(unavailable());
            }
            callback.tap = tap;
            let source = unsafe { CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0) };
            if source.is_null() {
                unsafe {
                    CFMachPortInvalidate(tap);
                    CFRelease(tap);
                }
                return Err(unavailable());
            }
            unsafe {
                CFRunLoopAddSource(CFRunLoopGetMain(), source, kCFRunLoopCommonModes);
            }
            *slot.borrow_mut() = Some(Monitor {
                tap,
                source,
                callback,
            });
            Ok(())
        })
    });
    if result.is_ok() {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let _ = app.run_on_main_thread(move || {
                MONITOR.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    if let Some(m) = slot.as_mut() {
                        if m.callback.edit_id == edit_id && m.callback.capture_id == capture_id {
                            m.callback.finish(Outcome::Cancelled);
                            slot.take();
                        }
                    }
                });
            });
        });
    }
    result
}

pub(crate) fn stop(app: &tauri::AppHandle, edit_id: Uuid, capture_id: Option<Uuid>) -> Result<()> {
    crate::fn_shortcut::main_thread(app, move || {
        MONITOR.with(|slot| {
            let matches = slot.borrow().as_ref().is_some_and(|m| {
                m.callback.edit_id == edit_id
                    && capture_id.is_none_or(|id| m.callback.capture_id == id)
            });
            if matches {
                slot.borrow_mut().take();
            }
        });
        Ok(())
    })
}
// Called on the existing main-thread refresh, including when no keys arrive.
pub(crate) fn refresh() {
    MONITOR.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(m) = slot.as_mut() {
            if !m.callback.valid() {
                m.callback.finish(Outcome::Cancelled);
                slot.take();
            }
        }
    });
}
pub(crate) fn destroy() {
    MONITOR.with(|slot| {
        slot.borrow_mut().take();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn captures_fn_and_consumes_entire_combination_without_repeat() {
        let mut g = Gesture::default();
        assert_eq!(g.event(12, 63, 1 << 23), None);
        assert_eq!(
            g.event(12, 63, 0),
            Some(Outcome::Captured {
                accelerator: "Fn".into()
            })
        );
        assert_eq!(g.event(12, 59, 1 << 18), None);
        assert_eq!(g.event(10, 49, (1 << 18) | (1 << 19)), None);
        assert_eq!(g.event(10, 49, (1 << 18) | (1 << 19)), None);
        assert_eq!(g.event(11, 49, 1 << 18), None);
        assert_eq!(
            g.event(12, 59, 0),
            Some(Outcome::Captured {
                accelerator: "Control+Alt+Space".into()
            })
        );
    }
    #[test]
    fn rejects_bare_keys_fn_chords_and_multi_key_chords_and_handles_escape() {
        for (key, flags, expected) in [
            (0, 0, Outcome::Invalid),
            (53, 0, Outcome::Cancelled),
            (
                53,
                1 << 20,
                Outcome::Captured {
                    accelerator: "Super+Escape".into(),
                },
            ),
        ] {
            let mut g = Gesture::default();
            g.event(10, key, flags);
            let result = g.event(11, key, 0);
            assert_eq!(result, Some(expected));
        }
        let mut g = Gesture::default();
        g.event(12, 63, 1 << 23);
        g.event(10, 0, 1 << 23);
        g.event(11, 0, 1 << 23);
        assert_eq!(g.event(12, 63, 0), Some(Outcome::Invalid));
        let mut g = Gesture::default();
        g.event(10, 0, 1 << 20);
        g.event(10, 1, 1 << 20);
        g.event(11, 1, 1 << 20);
        g.event(11, 0, 1 << 20);
        assert_eq!(g.event(12, 55, 0), Some(Outcome::Invalid));
    }
    #[test]
    fn physical_codes_use_canonical_grammar_including_arrows_and_function_keys() {
        assert_eq!(
            accelerator(2, (1 << 17) | (1 << 20)),
            Some("Shift+Super+KeyD".into())
        );
        assert_eq!(
            accelerator(126, (1 << 18) | (1 << 23)),
            Some("Control+ArrowUp".into())
        );
        assert_eq!(accelerator(105, 1 << 19), Some("Alt+F13".into()));
        assert_eq!(accelerator(0, 1 << 17), None);
        assert_eq!(accelerator(999, 1 << 20), None);
    }
}
