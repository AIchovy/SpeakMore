//! Observe system/workspace state on NSWorkspace's own notification center.
//! No microphone, OS setting changes or synthetic OS events are performed here.
use crate::session_runtime::{Interruption, NativeInput};
use block2::RcBlock;
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_app_kit::{
    NSRunningApplication, NSWorkspace, NSWorkspaceApplicationKey,
    NSWorkspaceDidActivateApplicationNotification, NSWorkspaceDidWakeNotification,
    NSWorkspaceScreensDidSleepNotification, NSWorkspaceScreensDidWakeNotification,
    NSWorkspaceSessionDidBecomeActiveNotification, NSWorkspaceSessionDidResignActiveNotification,
    NSWorkspaceWillPowerOffNotification, NSWorkspaceWillSleepNotification,
};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFString, CFType};
use objc2_foundation::{MainThreadMarker, NSNotification, NSNotificationCenter, NSObjectProtocol};
use std::{cell::RefCell, ptr::NonNull, time::Duration};

/// How long input stays blocked after a power-off/logout notification.
#[cfg(not(test))]
const POWER_OFF_RELEASE: Duration = Duration::from_secs(30);
#[cfg(test)]
const POWER_OFF_RELEASE: Duration = Duration::from_millis(50);

struct Observers {
    center: Retained<NSNotificationCenter>,
    tokens: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
}
impl Drop for Observers {
    fn drop(&mut self) {
        for token in &self.tokens {
            // SAFETY: these are the retained observer tokens from this center;
            // callbacks only own thread-safe NativeInput, never a borrowed Cocoa object.
            unsafe {
                self.center.removeObserver((**token).as_ref());
            }
        }
    }
}
thread_local! { static OBSERVERS: RefCell<Option<Observers>> = const { RefCell::new(None) }; }

pub(crate) fn setup(input: NativeInput) {
    MainThreadMarker::new().expect("Workspace observers must be installed on the main thread");
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    let observers = observe(center, input.clone());
    let generation = input.target_generation();
    let identity = NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .as_deref()
        .and_then(crate::applications::mac::identity);
    input.initialize_application(generation, identity);
    // setup may run after the launch-time inactive-session notification. Query
    // the public CGSession keys after registering; never log the dictionary,
    // which also contains identity data. This is not a screen-lock detector.
    input.initialize_session(|| {
        console_active(objc2_core_graphics::CGSessionCopyCurrentDictionary().as_deref())
    });
    OBSERVERS.with(|slot| *slot.borrow_mut() = Some(observers));
}

fn console_active(dictionary: Option<&CFDictionary>) -> bool {
    let Some(dictionary) = dictionary else {
        return false;
    };
    // These spellings come from SDK CGSession.h's kCGSessionOnConsoleKey and
    // kCGSessionLoginDoneKey macros. Missing/incorrect types fail closed.
    ["kCGSSessionOnConsoleKey", "kCGSessionLoginDoneKey"]
        .iter()
        .all(|key| {
            let key = CFString::from_str(key);
            // SAFETY: CGSession values (and our test dictionaries) are CF objects;
            // the retained dictionary owns their lifetime. Check CFBoolean's type.
            unsafe {
                dictionary
                    .value((&*key as *const CFString).cast())
                    .cast::<CFType>()
                    .as_ref()
            }
            .and_then(|value| value.downcast_ref::<CFBoolean>())
            .is_some_and(CFBoolean::as_bool)
        })
}

fn observe(center: Retained<NSNotificationCenter>, input: NativeInput) -> Observers {
    let mut observers = Observers {
        center,
        tokens: vec![],
    };
    let target_input = input.clone();
    let activated = RcBlock::new(move |notification: NonNull<NSNotification>| {
        // Only the fixed bundle ID leaves this callback; never record titles,
        // paths, userInfo dictionaries or application activation history.
        let identity = unsafe {
            notification
                .as_ref()
                .userInfo()
                .and_then(|info| info.objectForKey(NSWorkspaceApplicationKey))
        }
        .and_then(|value| {
            value
                .downcast_ref::<NSRunningApplication>()
                .and_then(crate::applications::mac::identity)
        });
        target_input.application_activated(identity);
    });
    let token = unsafe {
        observers.center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceDidActivateApplicationNotification),
            None,
            None,
            &activated,
        )
    };
    observers.tokens.push(token);
    // SAFETY: SDK notification-name constants; no mutable Objective-C globals.
    let notifications = unsafe {
        [
            (
                NSWorkspaceWillSleepNotification,
                Interruption::SystemSleep,
                true,
            ),
            (
                NSWorkspaceDidWakeNotification,
                Interruption::SystemSleep,
                false,
            ),
            (
                NSWorkspaceScreensDidSleepNotification,
                Interruption::DisplaySleep,
                true,
            ),
            (
                NSWorkspaceScreensDidWakeNotification,
                Interruption::DisplaySleep,
                false,
            ),
            (
                NSWorkspaceSessionDidResignActiveNotification,
                Interruption::InactiveSession,
                true,
            ),
            (
                NSWorkspaceSessionDidBecomeActiveNotification,
                Interruption::InactiveSession,
                false,
            ),
            (
                NSWorkspaceWillPowerOffNotification,
                Interruption::PowerOff,
                true,
            ),
        ]
    };
    for (name, reason, blocked) in notifications {
        let input = input.clone();
        let block = RcBlock::new(move |_: NonNull<NSNotification>| {
            let epoch = input.interrupt(reason, blocked);
            // Apple permits delaying sleep while handling willSleepNotification
            // (up to 30 s). Bound our synchronous drain wait to 3 s. The actor
            // acknowledges source release and child reaping without a UI callback.
            if blocked
                && matches!(reason, Interruption::SystemSleep | Interruption::PowerOff)
                && !input.wait_quiesced(epoch, Duration::from_secs(3))
            {
                eprintln!("SpeakMore: system interruption cleanup exceeded deadline");
            }
            // Power-off has no "cancelled" notification. A proceeding logout
            // terminates us well before the release, so the model is not reloaded
            // mid-logout; one another app cancels must not block input forever.
            if matches!(reason, Interruption::PowerOff) {
                let input = input.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(POWER_OFF_RELEASE);
                    input.interrupt(reason, false);
                });
            }
        });
        // SAFETY: nil queue means synchronous notification delivery; callback
        // captures only Send + Sync state. Foundation copies the block and the
        // returned token is retained until explicit removal on application exit.
        let token = unsafe {
            observers.center.addObserverForName_object_queue_usingBlock(
                Some(name),
                None,
                None,
                &block,
            )
        };
        observers.tokens.push(token);
    }
    observers
}

pub(crate) fn destroy() {
    OBSERVERS.with(|slot| {
        slot.borrow_mut().take();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_console_check_requires_both_typed_true_values() {
        let on_console = CFString::from_str("kCGSSessionOnConsoleKey");
        let logged_in = CFString::from_str("kCGSessionLoginDoneKey");
        let keys = [&*on_console, &*logged_in];
        let yes: &CFType = CFBoolean::new(true).as_ref();
        let no: &CFType = CFBoolean::new(false).as_ref();
        let text = CFString::from_str("true");
        let wrong: &CFType = text.as_ref();
        for (values, expected) in [
            ([yes, yes], true),
            ([yes, no], false),
            ([no, yes], false),
            ([yes, wrong], false),
        ] {
            let dict = CFDictionary::from_slices(&keys, &values);
            assert_eq!(console_active(Some(dict.as_opaque())), expected);
        }
        assert!(!console_active(None));
        assert!(!console_active(Some(
            CFDictionary::<CFString, CFType>::empty().as_opaque()
        )));
    }

    #[test]
    fn app_activation_changes_target_generation_without_cancelling_audio() {
        let center = NSNotificationCenter::new();
        let input = NativeInput::default();
        let observers = observe(center.clone(), input.clone());
        unsafe {
            center.postNotificationName_object(NSWorkspaceDidActivateApplicationNotification, None);
            center.postNotificationName_object(NSWorkspaceDidActivateApplicationNotification, None);
        }
        assert_eq!(input.target_generation(), 2);
        assert_eq!(input.lifecycle().epoch, 0);
        assert!(!input.lifecycle().blocked());
        drop(observers);
        unsafe {
            center.postNotificationName_object(NSWorkspaceDidActivateApplicationNotification, None);
        }
        assert_eq!(input.target_generation(), 2);
    }
    #[test]
    fn native_notification_registration_pairs_reasons_and_removes_tokens() {
        // A PRIVATE center: never post synthetic OS events to NSWorkspace or
        // the process-wide center. This checks the actual Foundation adapter.
        let center = NSNotificationCenter::new();
        let input = NativeInput::default();
        let observers = observe(center.clone(), input.clone());
        // Unit tests only: pre-acknowledge the drain, tested with an actual actor
        // in session_runtime. This test verifies names, callbacks and ownership.
        input.acknowledge_quiesced(100);
        unsafe {
            center.postNotificationName_object(NSWorkspaceScreensDidSleepNotification, None);
            center.postNotificationName_object(NSWorkspaceSessionDidResignActiveNotification, None);
            center.postNotificationName_object(NSWorkspaceWillSleepNotification, None);
            assert_eq!(input.lifecycle().epoch, 3);
            center.postNotificationName_object(NSWorkspaceDidWakeNotification, None);
            center.postNotificationName_object(NSWorkspaceScreensDidWakeNotification, None);
            assert!(input.lifecycle().blocked());
            center.postNotificationName_object(NSWorkspaceSessionDidBecomeActiveNotification, None);
            assert!(!input.lifecycle().blocked());
            let before = input.lifecycle().epoch;
            center.postNotificationName_object(NSWorkspaceWillPowerOffNotification, None);
            assert_eq!(input.lifecycle().epoch, before + 1);
            assert!(input.lifecycle().blocked());
            let released = std::time::Instant::now() + Duration::from_secs(2);
            while input.lifecycle().blocked() && std::time::Instant::now() < released {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(!input.lifecycle().blocked());
            assert_eq!(input.lifecycle().epoch, before + 1);
        }
        let epoch = input.lifecycle().epoch;
        drop(observers);
        unsafe {
            center.postNotificationName_object(NSWorkspaceWillSleepNotification, None);
        }
        assert_eq!(input.lifecycle().epoch, epoch);
    }
}
