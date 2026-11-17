//! Keep Tauri's hidden host for WebView/IPC ownership, and present its content
//! in a nonactivating AppKit panel that can join other applications' Spaces.
use objc2::{define_class, msg_send, rc::Retained, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSColor, NSEvent, NSEventType, NSPanel, NSScreen, NSWindow,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSObjectProtocol, NSPoint, NSSize};
use std::cell::RefCell;
use tauri::{Manager, WebviewWindow};

define_class!(
    #[unsafe(super(NSPanel))]
    #[thread_kind = MainThreadOnly]
    #[name = "SpeakMoreCaptionPanel"]
    struct CaptionPanel;
    unsafe impl NSObjectProtocol for CaptionPanel {}
    impl CaptionPanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool { false }
        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool { false }
    }
);

struct Panel {
    panel: Retained<CaptionPanel>,
    host: Retained<NSWindow>,
    placed: bool,
}
impl Drop for Panel {
    fn drop(&mut self) {
        self.panel.orderOut(None);
        let content = self.panel.contentView();
        self.panel.setContentView(None);
        self.host.setContentView(content.as_deref());
        // Tauri, not AppKit, owns the host window's teardown.
    }
}
thread_local! { static CAPTIONS: RefCell<Option<Panel>> = const { RefCell::new(None) }; }

pub(crate) fn create(window: &WebviewWindow) -> tauri::Result<()> {
    let mtm = MainThreadMarker::new().expect("Caption panels must be created on the main thread");
    // ns_window() returns a borrowed Tauri-owned NSWindow; keep it alive while
    // its content is temporarily hosted by our independently owned NSPanel.
    let host = unsafe { Retained::retain(window.ns_window()?.cast::<NSWindow>()) }
        .expect("Tauri returned a null caption window");
    let panel: Retained<CaptionPanel> = unsafe {
        msg_send![super(CaptionPanel::alloc(mtm).set_ivars(())),
            initWithContentRect: host.frame(),
            styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel | NSWindowStyleMask::Resizable,
            backing: NSBackingStoreType::Buffered,
            defer: false]
    };
    unsafe {
        panel.setReleasedWhenClosed(false);
    }
    panel.setTitle(&host.title());
    panel.setFloatingPanel(true);
    panel.setBecomesKeyOnlyIfNeeded(true);
    panel.setHidesOnDeactivate(false);
    panel.setLevel(3);
    panel.setOpaque(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setHasShadow(false);
    panel.setContentMinSize(NSSize::new(420.0, 120.0));
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::CanJoinAllApplications
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    let content = host.contentView();
    host.setContentView(None);
    panel.setContentView(content.as_deref());
    CAPTIONS.with(|slot| {
        *slot.borrow_mut() = Some(Panel {
            panel,
            host,
            placed: false,
        })
    });
    Ok(())
}

pub(crate) enum Action {
    Show,
    Hide,
    Drag,
    Appearance { height: f64, passthrough: bool },
}
pub(crate) fn dispatch(app: &tauri::AppHandle, action: Action) -> tauri::Result<()> {
    app.run_on_main_thread(move || {
        let mtm = MainThreadMarker::new().expect("Caption actions run on the main thread");
        CAPTIONS.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(state) = slot.as_mut() else { return };
            match action {
                Action::Show => {
                    if !state.placed {
                        let pointer = NSEvent::mouseLocation();
                        let screens = NSScreen::screens(mtm);
                        let screen = screens.iter().find(|s| {
                            let f = s.frame();
                            pointer.x >= f.origin.x && pointer.x < f.origin.x + f.size.width
                                && pointer.y >= f.origin.y && pointer.y < f.origin.y + f.size.height
                        }).or_else(|| NSScreen::mainScreen(mtm));
                        if let Some(screen) = screen {
                            let frame = screen.visibleFrame();
                            let size = state.panel.frame().size;
                            state.panel.setFrameOrigin(NSPoint::new(
                                frame.origin.x + ((frame.size.width - size.width) / 2.0).max(0.0),
                                frame.origin.y + 48.0,
                            ));
                        }
                        state.placed = true;
                    }
                    // Do not activate SpeakMore or switch back to its main Space.
                    state.panel.orderFrontRegardless();
                }
                Action::Hide => state.panel.orderOut(None),
                Action::Appearance { height, passthrough } => {
                    let frame = state.panel.frame();
                    state.panel.setContentSize(NSSize::new(frame.size.width, height));
                    state.panel.setFrameOrigin(frame.origin);
                    state.panel.setIgnoresMouseEvents(passthrough);
                }
                Action::Drag => {
                    if NSEvent::pressedMouseButtons() & 1 == 0 { return; }
                    let Some(event) = NSApplication::sharedApplication(mtm).currentEvent() else { return };
                    // IPC can run during an AppKit-defined event. Start a native
                    // drag with a mouse event belonging to the visible panel,
                    // rather than dragging the now-hidden Tauri host.
                    let location = state.panel.convertPointFromScreen(NSEvent::mouseLocation());
                    let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                        NSEventType::LeftMouseDown, location, event.modifierFlags(), event.timestamp(),
                        state.panel.windowNumber(), None, 0, 1, 1.0,
                    );
                    if let Some(event) = event {
                        // AppKit runs a nested event loop while dragging. Release
                        // the thread-local borrow before entering it.
                        let panel = state.panel.clone();
                        drop(slot);
                        panel.performWindowDragWithEvent(&event);
                    }
                }
            }
        });
    })
}

pub(crate) fn destroy(app: &tauri::AppHandle) {
    CAPTIONS.with(|slot| {
        slot.borrow_mut().take();
    });
    if let Some(window) = app.get_webview_window("captions") {
        let _ = window.destroy();
    }
}
