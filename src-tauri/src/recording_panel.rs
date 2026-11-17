//! An actual NSPanel owns the HUD content view. The hidden Tauri host remains
//! alive for WebView/IPC ownership; its Objective-C class is never changed.
//! All Cocoa objects remain in main-thread local storage, including teardown.
use objc2::{define_class, msg_send, rc::Retained, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSEvent, NSPanel, NSScreen, NSWindow, NSWindowCollectionBehavior,
    NSWindowStyleMask,
};
use objc2_foundation::{ns_string, MainThreadMarker, NSObjectProtocol, NSPoint, NSRect, NSSize};
use std::cell::RefCell;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

define_class!(
    #[unsafe(super(NSPanel))]
    #[thread_kind = MainThreadOnly]
    #[name = "SpeakMoreRecordingPanel"]
    struct RecordingPanel;
    unsafe impl NSObjectProtocol for RecordingPanel {}
    impl RecordingPanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool { false }
        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool { false }
    }
);

struct Panel {
    panel: Retained<RecordingPanel>,
    host: Retained<NSWindow>,
}
impl Drop for Panel {
    fn drop(&mut self) {
        self.panel.orderOut(None);
        let view = self.panel.contentView();
        self.panel.setContentView(None);
        self.host.setContentView(view.as_deref());
        // Never close/release a Tauri-owned NSWindow from Cocoa code.
    }
}
thread_local! { static HUD: RefCell<Option<Panel>> = const { RefCell::new(None) }; }

pub(crate) fn create(app: &tauri::AppHandle) -> tauri::Result<()> {
    let mtm = MainThreadMarker::new().expect("HUD creation must run on the main thread");
    let host = WebviewWindowBuilder::new(app, "hud", WebviewUrl::App("hud.html".into()))
        .title("SpeakMore Recording")
        .inner_size(280.0, 72.0)
        .decorations(false)
        .resizable(false)
        .visible(false)
        .focused(false)
        .focusable(false)
        .transparent(true)
        .skip_taskbar(true)
        .build()?;
    // SAFETY: Tauri documents ns_window() as an NSWindow pointer; retain while
    // hosting its content. The Tauri host itself stays alive until application exit.
    let host = unsafe { Retained::retain(host.ns_window()?.cast::<NSWindow>()) }
        .expect("Tauri returned a null NSWindow");
    let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(280.0, 72.0));
    let panel: Retained<RecordingPanel> = unsafe {
        msg_send![super(RecordingPanel::alloc(mtm).set_ivars(())),
            initWithContentRect: rect,
            styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            backing: NSBackingStoreType::Buffered,
            defer: false]
    };
    // Retained<RecordingPanel> owns lifetime; AppKit must not release on close.
    unsafe {
        panel.setReleasedWhenClosed(false);
    }
    panel.setFloatingPanel(true);
    panel.setTitle(ns_string!("SpeakMore Recording"));
    panel.setBecomesKeyOnlyIfNeeded(true);
    panel.setHidesOnDeactivate(false);
    panel.setLevel(3); // NSFloatingWindowLevel; never covers menus or security dialogs.
    panel.setOpaque(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setHasShadow(false);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    let content = host.contentView();
    host.setContentView(None);
    panel.setContentView(content.as_deref());
    HUD.with(|slot| *slot.borrow_mut() = Some(Panel { panel, host }));
    Ok(())
}

pub(crate) fn visible(show: bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    HUD.with(|slot| {
        let state = slot.borrow();
        let Some(state) = state.as_ref() else {
            return;
        };
        if show && !state.panel.isVisible() {
            // Until target observation is connected, anchor to the screen under
            // the pointer when recording starts. App-local keyWindow is not the
            // foreground window of another application.
            let pointer = NSEvent::mouseLocation();
            let screens = NSScreen::screens(mtm);
            let screen = screens
                .iter()
                .find(|screen| {
                    let f = screen.frame();
                    pointer.x >= f.origin.x
                        && pointer.x < f.origin.x + f.size.width
                        && pointer.y >= f.origin.y
                        && pointer.y < f.origin.y + f.size.height
                })
                .or_else(|| NSScreen::mainScreen(mtm));
            if let Some(screen) = screen {
                let frame = screen.visibleFrame();
                state.panel.setFrameOrigin(NSPoint::new(
                    frame.origin.x + (frame.size.width - 280.0) / 2.0,
                    frame.origin.y + 16.0,
                ));
            }
            state.panel.orderFrontRegardless();
        } else if !show && state.panel.isVisible() {
            state.panel.orderOut(None);
        }
    });
}

pub(crate) fn destroy(app: &tauri::AppHandle) {
    HUD.with(|slot| {
        slot.borrow_mut().take();
    });
    if let Some(window) = app.get_webview_window("hud") {
        let _ = window.destroy();
    }
}
