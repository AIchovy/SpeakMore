use uuid::Uuid;

#[derive(Default)]
struct Presentation {
    current: Option<Uuid>,
    hidden: Option<Uuid>,
    ready: bool,
    editing: bool,
}
impl Presentation {
    fn sync(&mut self, current: Option<Uuid>) -> bool {
        if self.current != current {
            self.current = current;
            self.hidden = None;
            self.editing = false;
        }
        self.ready && current.is_some() && self.hidden != current
    }
    fn hide(&mut self, id: Uuid) {
        if self.current == Some(id) {
            self.hidden = Some(id);
            self.editing = false;
        }
    }
    fn compose(&mut self, id: Uuid, editing: bool) {
        if self.current == Some(id) && self.hidden != Some(id) {
            self.editing = editing;
        }
    }
}
pub(crate) enum Action {
    Ready,
    Show(Uuid),
    Hide(Uuid),
    Drag,
    Compose(Uuid, bool),
}

#[cfg(target_os = "macos")]
pub(crate) use mac::{create, destroy, dispatch, sync};

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use objc2::{define_class, msg_send, rc::Retained, DefinedClass, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSBackingStoreType, NSColor, NSEvent, NSEventType, NSPanel, NSScreen,
        NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
    };
    use objc2_foundation::{MainThreadMarker, NSObjectProtocol, NSPoint, NSRect, NSSize};
    use std::cell::{Cell, RefCell};
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

    #[derive(Default)]
    struct PanelIvars {
        accepts_input: Cell<bool>,
    }
    define_class!(
        #[unsafe(super(NSPanel))]
        #[thread_kind = MainThreadOnly]
        #[name = "SpeakMoreMeetingRecordingPanel"]
        #[ivars = PanelIvars]
        struct MeetingPanel;
        unsafe impl NSObjectProtocol for MeetingPanel {}
        impl MeetingPanel {
            #[unsafe(method(canBecomeKeyWindow))]
            fn can_become_key(&self) -> bool { self.ivars().accepts_input.get() }
            #[unsafe(method(canBecomeMainWindow))]
            fn can_become_main(&self) -> bool { false }
        }
    );
    struct Panel {
        panel: Retained<MeetingPanel>,
        host: Retained<NSWindow>,
        presentation: Presentation,
        placed: bool,
    }
    impl Drop for Panel {
        fn drop(&mut self) {
            self.panel.orderOut(None);
            let content = self.panel.contentView();
            self.panel.setContentView(None);
            self.host.setContentView(content.as_deref());
        }
    }
    thread_local! { static PANEL: RefCell<Option<Panel>> = const { RefCell::new(None) }; }
    pub(crate) fn create(app: &tauri::AppHandle) -> tauri::Result<()> {
        let mtm = MainThreadMarker::new().expect("Meeting panels are created on the main thread");
        let window = WebviewWindowBuilder::new(
            app,
            "meeting-hud",
            WebviewUrl::App("meeting-hud.html".into()),
        )
        .title("SpeakMore Meeting")
        .inner_size(380.0, 196.0)
        .decorations(false)
        .resizable(false)
        .visible(false)
        .focused(false)
        .focusable(false)
        .transparent(true)
        .skip_taskbar(true)
        .build()?;
        let host = unsafe { Retained::retain(window.ns_window()?.cast::<NSWindow>()) }
            .expect("Tauri returned a null meeting window");
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(380.0, 196.0));
        let panel: Retained<MeetingPanel> = unsafe {
            msg_send![super(MeetingPanel::alloc(mtm).set_ivars(PanelIvars::default())),
                initWithContentRect: rect,
                styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                backing: NSBackingStoreType::Buffered, defer: false]
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
        PANEL.with(|slot| {
            *slot.borrow_mut() = Some(Panel {
                panel,
                host,
                presentation: Presentation::default(),
                placed: false,
            })
        });
        Ok(())
    }
    fn apply(state: &mut Panel, current: Option<Uuid>) {
        let visible = state.presentation.sync(current);
        state
            .panel
            .ivars()
            .accepts_input
            .set(visible && state.presentation.editing);
        if !state.presentation.editing && state.panel.isKeyWindow() {
            state.panel.resignKeyWindow();
        }
        let height = if state.presentation.editing {
            324.0
        } else {
            196.0
        };
        let frame = state.panel.frame();
        if frame.size.height != height {
            state.panel.setContentSize(NSSize::new(380.0, height));
            let y = frame.origin.y + frame.size.height - height;
            let y = state.panel.screen().map_or(y, |screen| {
                let bounds = screen.visibleFrame();
                y.clamp(
                    bounds.origin.y,
                    bounds.origin.y + (bounds.size.height - height).max(0.0),
                )
            });
            state.panel.setFrameOrigin(NSPoint::new(frame.origin.x, y));
        }
        if !visible {
            state.panel.orderOut(None);
            return;
        }
        if !state.placed {
            let mtm = MainThreadMarker::new().expect("Meeting panel layout is on the main thread");
            let pointer = NSEvent::mouseLocation();
            let screens = NSScreen::screens(mtm);
            let screen = screens
                .iter()
                .find(|screen| {
                    let frame = screen.frame();
                    pointer.x >= frame.origin.x
                        && pointer.x < frame.origin.x + frame.size.width
                        && pointer.y >= frame.origin.y
                        && pointer.y < frame.origin.y + frame.size.height
                })
                .or_else(|| NSScreen::mainScreen(mtm));
            if let Some(screen) = screen {
                let frame = screen.visibleFrame();
                state.panel.setFrameOrigin(NSPoint::new(
                    frame.origin.x + (frame.size.width - 404.0).max(0.0),
                    frame.origin.y + 100.0,
                ));
            }
            state.placed = true;
        }
        if !state.panel.isVisible() {
            state.panel.orderFrontRegardless();
        }
    }
    pub(crate) fn sync(current: Option<Uuid>) {
        PANEL.with(|slot| {
            if let Some(state) = slot.borrow_mut().as_mut() {
                apply(state, current);
            }
        });
    }
    pub(crate) fn dispatch(app: &tauri::AppHandle, action: Action) -> tauri::Result<()> {
        if app.get_webview_window("meeting-hud").is_none() {
            return Err(tauri::Error::AssetNotFound("meeting-hud".into()));
        }
        let app = app.clone();
        let handle = app.clone();
        app.run_on_main_thread(move || {
            let current = handle.try_state::<crate::meetings::Managed>()
                .and_then(|m| m.0.as_ref().ok().and_then(|s| s.recording_status()))
                .map(|(id, _, _)| id);
            PANEL.with(|slot| {
                let mut slot = slot.borrow_mut();
                let Some(state) = slot.as_mut() else { return };
                match action {
                    Action::Ready => state.presentation.ready = true,
                    Action::Show(id) if current == Some(id) => state.presentation.hidden = None,
                    Action::Hide(id) => state.presentation.hide(id),
                    Action::Compose(id, editing) if current == Some(id) => {
                        state.presentation.compose(id, editing);
                        apply(state, current);
                        if state.presentation.editing { state.panel.makeKeyWindow(); }
                        return;
                    }
                    Action::Drag => {
                        if NSEvent::pressedMouseButtons() & 1 == 0 { return; }
                        let mtm = MainThreadMarker::new().expect("Meeting panel actions run on the main thread");
                        let Some(event) = NSApplication::sharedApplication(mtm).currentEvent() else { return };
                        let location = state.panel.convertPointFromScreen(NSEvent::mouseLocation());
                        let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                            NSEventType::LeftMouseDown, location, event.modifierFlags(), event.timestamp(),
                            state.panel.windowNumber(), None, 0, 1, 1.0,
                        );
                        if let Some(event) = event {
                            let panel = state.panel.clone();
                            drop(slot);
                            panel.performWindowDragWithEvent(&event);
                        }
                        return;
                    }
                    _ => {}
                }
                apply(state, current);
            });
        })
    }
    pub(crate) fn destroy(app: &tauri::AppHandle) {
        PANEL.with(|slot| {
            slot.borrow_mut().take();
        });
        if let Some(window) = app.get_webview_window("meeting-hud") {
            let _ = window.destroy();
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn create(_: &tauri::AppHandle) -> tauri::Result<()> {
    Err(tauri::Error::AssetNotFound("meeting-hud".into()))
}
#[cfg(not(target_os = "macos"))]
pub(crate) fn dispatch(_: &tauri::AppHandle, _: Action) -> tauri::Result<()> {
    Err(tauri::Error::AssetNotFound("meeting-hud".into()))
}
#[cfg(not(target_os = "macos"))]
pub(crate) fn sync(_: Option<Uuid>) {}
#[cfg(not(target_os = "macos"))]
pub(crate) fn destroy(_: &tauri::AppHandle) {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn panel_waits_for_content_and_hiding_does_not_end_or_reopen_the_meeting() {
        let id = Uuid::new_v4();
        let mut p = Presentation::default();
        assert!(!p.sync(Some(id)));
        p.ready = true;
        assert!(p.sync(Some(id)));
        p.hide(id);
        assert!(!p.sync(Some(id)));
        assert_eq!(p.current, Some(id));
        p.hidden = None;
        assert!(p.sync(Some(id)));
        assert!(!p.sync(None));
        assert!(p.sync(Some(id)));
    }
    #[test]
    fn late_hide_of_previous_meeting_does_not_hide_the_next_recording() {
        let first = Uuid::new_v4();
        let next = Uuid::new_v4();
        let mut p = Presentation {
            ready: true,
            ..Default::default()
        };
        assert!(p.sync(Some(first)));
        p.hide(first);
        assert!(p.sync(Some(next)));
        p.hide(first);
        assert!(p.sync(Some(next)));
    }
    #[test]
    fn typing_is_opt_in_and_resets_when_hidden_or_recording_changes() {
        let first = Uuid::new_v4();
        let next = Uuid::new_v4();
        let mut p = Presentation {
            ready: true,
            ..Default::default()
        };
        p.sync(Some(first));
        assert!(!p.editing);
        p.compose(first, true);
        assert!(p.editing);
        p.hide(first);
        assert!(!p.editing);
        p.compose(first, true);
        assert!(!p.editing);
        p.sync(Some(next));
        p.compose(first, true);
        assert!(!p.editing);
        p.compose(next, true);
        assert!(p.editing);
        p.compose(next, false);
        assert!(!p.editing);
        p.compose(next, true);
        p.sync(None);
        assert!(!p.editing);
    }
}
