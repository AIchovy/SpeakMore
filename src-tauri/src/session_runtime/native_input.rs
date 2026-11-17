//! Native input is a latest-state latch, separate from the bounded IPC mailbox.
//! A release cannot be dropped behind WebView commands. A tap fully completed
//! before the actor observes it never starts a delayed microphone capture.
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Clone)]
pub(crate) struct Press {
    pub cycle: Uuid,
    pub at: Instant,
    pub control_revision: u64,
    pub target_generation: u64,
    pub app_identity: Option<String>,
    pub epoch: u64,
    pub manual_only: bool,
}
struct Editor {
    id: Uuid,
    expires: Instant,
    committing: bool,
}
#[derive(Default)]
struct State {
    down: Option<Press>,
    toggle: Option<(Press, bool)>,
    toggle_down: bool,
    shortcut_key_downs: u32,
    stop: Option<(Uuid, bool)>,
    cancelled_session: Option<Uuid>,
    stop_revision: u64,
    lifecycle: Lifecycle,
    require_release: bool,
    quiesced_epoch: u64,
    editor: Option<Editor>,
    control_revision: u64,
    cancel_down: bool,
    hold_disabled: bool,
    target_generation: u64,
    app_identity: Option<String>,
}
impl State {
    fn expire_editor(&mut self) {
        if self
            .editor
            .as_ref()
            .is_some_and(|e| !e.committing && e.expires <= Instant::now())
        {
            self.editor = None;
            self.down = None;
            self.toggle = None;
            self.require_release = true;
        }
    }
    fn interrupt(&mut self, reason: Interruption, blocked: bool) -> u64 {
        let bit = reason as u8;
        if blocked && self.lifecycle.reasons & bit == 0 {
            self.lifecycle.epoch += 1;
            self.lifecycle.reasons |= bit;
            // A sleeping display only invalidates input. Every other reason
            // also retires the worker and revalidates the model on resume.
            if !matches!(reason, Interruption::DisplaySleep) {
                self.lifecycle.teardown = self.lifecycle.epoch;
            }
            self.down = None;
            self.toggle = None;
            self.require_release = true;
        } else if !blocked {
            self.lifecycle.reasons &= !bit;
        }
        self.lifecycle.epoch
    }
}
#[derive(Clone, Copy)]
pub(crate) enum Interruption {
    SystemSleep = 1,
    DisplaySleep = 2,
    InactiveSession = 4,
    PowerOff = 8,
    Shutdown = 16,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Lifecycle {
    pub epoch: u64,
    reasons: u8,
    teardown: u64,
}
impl Lifecycle {
    pub fn blocked(self) -> bool {
        self.reasons != 0
    }
    /// Whether an interruption after `epoch` requires reaping the worker.
    pub(super) fn teardown_after(self, epoch: u64) -> bool {
        self.teardown > epoch
    }
}
pub(super) struct InputSnapshot {
    pub press: Option<Press>,
    pub toggle: Option<(Press, bool)>,
    pub stop: Option<(Uuid, bool)>,
    pub lifecycle: Lifecycle,
    pub require_release: bool,
    pub editing: bool,
}
#[derive(Clone, Default)]
pub(crate) struct NativeInput(Arc<Shared>);
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    quiescence: Condvar,
    wake: tokio::sync::Notify,
}
impl NativeInput {
    /// The idle actor polls slowly; input and lifecycle changes wake it at once.
    fn wake(&self) {
        self.0.wake.notify_one();
    }
    pub(super) async fn changed(&self) {
        self.0.wake.notified().await
    }
    #[cfg(test)]
    pub(crate) fn target_changed(&self) {
        self.application_activated(None);
    }
    pub(crate) fn application_activated(&self, identity: Option<String>) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.target_generation += 1;
        state.app_identity =
            identity.and_then(|v| speakmore_core::app_rules::macos_identity(&v).ok());
    }
    pub(crate) fn initialize_application(&self, generation: u64, identity: Option<String>) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.target_generation == generation {
            state.app_identity =
                identity.and_then(|v| speakmore_core::app_rules::macos_identity(&v).ok());
        }
    }
    pub(crate) fn target_generation(&self) -> u64 {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .target_generation
    }
    pub(super) fn permissions_changed(&self, cancel_active: bool) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        // Permission changes invalidate already queued starts and held keys.
        // Missing permission does not block model download/selection or editing.
        state.control_revision += 1;
        state.down = None;
        state.toggle = None;
        state.require_release = true;
        if cancel_active {
            state.lifecycle.epoch += 1;
            state.lifecycle.teardown = state.lifecycle.epoch;
        }
    }
    #[cfg(test)]
    pub(crate) fn key(&self, down: bool) {
        self.key_with_origin(down, None);
    }
    #[cfg(target_os = "macos")]
    pub(crate) fn native_origin(&self, manual_only: bool) -> Press {
        let at = Instant::now();
        let (target_generation, control_revision, epoch) = {
            let state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.target_generation,
                state.control_revision,
                state.lifecycle.epoch,
            )
        };
        let identity = objc2_app_kit::NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .as_deref()
            .and_then(crate::applications::mac::identity);
        let app_identity = if self.target_generation() == target_generation
            && at.elapsed() <= Duration::from_millis(100)
        {
            identity
        } else {
            None
        };
        Press {
            cycle: Uuid::new_v4(),
            at,
            control_revision,
            target_generation,
            app_identity,
            epoch,
            manual_only,
        }
    }
    /// Ordinary combinations toggle only on a fresh key-down. Releases and
    /// auto-repeat never stop a recording; the next press does.
    pub(crate) fn native_key(&self, down: bool) {
        let pressed = {
            let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
            if down {
                state.shortcut_key_downs = state.shortcut_key_downs.wrapping_add(1);
            }
            let pressed = down && !state.toggle_down;
            state.toggle_down = down;
            if !down {
                state.require_release = false;
            }
            pressed
        };
        if pressed {
            self.native_toggle();
        } else if !down {
            self.wake();
        }
    }
    #[cfg(target_os = "macos")]
    pub(crate) fn shortcut_key_downs(&self) -> u32 {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .shortcut_key_downs
    }
    /// Called once for a completed native tap. Unlike hold input, releasing the
    /// physical key must not discard a pending toggle before the actor sees it.
    pub(crate) fn native_toggle(&self) {
        #[cfg(target_os = "macos")]
        let origin = self.native_origin(false);
        #[cfg(not(target_os = "macos"))]
        let origin = {
            let state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
            Press {
                cycle: Uuid::new_v4(),
                at: Instant::now(),
                control_revision: state.control_revision,
                target_generation: state.target_generation,
                app_identity: state.app_identity.clone(),
                epoch: state.lifecycle.epoch,
                manual_only: false,
            }
        };
        self.toggle_with_origin(origin);
    }
    fn toggle_with_origin(&self, origin: Press) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.expire_editor();
        if state.lifecycle.blocked()
            || state.editor.is_some()
            || state.hold_disabled
            || origin.epoch != state.lifecycle.epoch
            || origin.control_revision != state.control_revision
        {
            return;
        }
        state.require_release = false; // a complete press/release was observed
                                       // Preserve a rapid second tap. At idle it cancels the pending start;
                                       // during recording it must still finish, never swallow the stop.
        if let Some((_, multiple)) = state.toggle.as_mut() {
            *multiple = true;
        } else {
            state.toggle = Some((origin, false));
        }
        self.wake();
    }
    #[cfg(test)]
    fn key_with_origin(&self, down: bool, origin: Option<Press>) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.expire_editor();
        if down {
            if state.lifecycle.blocked()
                || state.require_release
                || state.editor.is_some()
                || state.hold_disabled
            {
                state.require_release = true;
                return;
            }
            let control_revision = state.control_revision;
            let target_generation = state.target_generation;
            let app_identity = state.app_identity.clone();
            let epoch = state.lifecycle.epoch;
            state.down.get_or_insert_with(|| {
                origin.unwrap_or_else(|| Press {
                    cycle: Uuid::new_v4(),
                    at: Instant::now(),
                    control_revision,
                    target_generation,
                    app_identity,
                    epoch,
                    manual_only: false,
                })
            });
        } else {
            state.down = None;
            state.require_release = false;
        }
        self.wake();
    }
    pub(crate) fn cancel_key(&self, down: bool, session: Option<Uuid>, revision: u64) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.expire_editor();
        let pressed = down && !state.cancel_down;
        state.cancel_down = down;
        if pressed && state.editor.is_none() && !state.lifecycle.blocked() {
            if let Some(id) = session.filter(|_| revision >= state.stop_revision) {
                state.stop_revision = revision;
                state.stop = Some((id, true));
                state.cancelled_session = Some(id);
                self.wake();
            }
        }
    }
    pub(crate) fn set_hold_enabled(&self, enabled: bool) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.hold_disabled = !enabled;
        state.toggle_down = false;
        state.down = None;
        state.toggle = None;
        self.wake();
    }
    pub(crate) fn release_commit(&self, id: Uuid) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(editor) = state.editor.as_mut().filter(|e| e.id == id) {
            editor.committing = false;
            editor.expires = Instant::now() + Duration::from_secs(30);
        }
        self.wake();
    }
    pub(crate) fn begin_edit(&self) -> speakmore_core::error::Result<Uuid> {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.expire_editor();
        if state.editor.is_some()
            || state.down.is_some()
            || state.toggle_down
            || state.lifecycle.blocked()
        {
            return Err(speakmore_core::error::AppError::busy());
        }
        let id = Uuid::new_v4();
        state.editor = Some(Editor {
            id,
            expires: Instant::now() + Duration::from_secs(30),
            committing: false,
        });
        state.control_revision += 1;
        state.toggle = None;
        Ok(id)
    }
    pub(crate) fn renew_edit(&self, id: Uuid) -> speakmore_core::error::Result<()> {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.expire_editor();
        if let Some(editor) = state.editor.as_mut().filter(|e| e.id == id) {
            editor.expires = Instant::now() + Duration::from_secs(30);
            Ok(())
        } else {
            Err(edit_expired())
        }
    }
    pub(crate) fn commit_edit(&self, id: Uuid) -> speakmore_core::error::Result<()> {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.expire_editor();
        if state.lifecycle.blocked() {
            return Err(super::interrupted());
        }
        if let Some(editor) = state
            .editor
            .as_mut()
            .filter(|e| e.id == id && !e.committing)
        {
            editor.committing = true;
            Ok(())
        } else {
            Err(edit_expired())
        }
    }
    pub(crate) fn end_edit(&self, id: Uuid, committed: bool) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if state
            .editor
            .as_ref()
            .is_some_and(|e| e.id == id && (committed || !e.committing))
        {
            state.editor = None;
            state.down = None;
        }
        self.wake();
    }
    #[cfg(target_os = "macos")]
    pub(crate) fn edit_matches(&self, id: Uuid) -> bool {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.expire_editor();
        !state.lifecycle.blocked()
            && state
                .editor
                .as_ref()
                .is_some_and(|e| e.id == id && !e.committing)
    }
    pub(crate) fn editing(&self) -> bool {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.expire_editor();
        state.editor.is_some()
    }
    pub(crate) fn control_revision(&self) -> u64 {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .control_revision
    }
    /// A resume clears only its own reason. The epoch survives even a complete
    /// suspend/resume pair between actor polls, invalidating queued start work.
    pub(crate) fn interrupt(&self, reason: Interruption, blocked: bool) -> u64 {
        let epoch = self
            .0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .interrupt(reason, blocked);
        self.wake();
        epoch
    }
    pub(crate) fn initialize_session(&self, active: impl FnOnce() -> bool) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        // Serialize the startup sample with notifications. The closure only
        // performs the synchronous CGSession query and never calls NativeInput.
        state.interrupt(Interruption::InactiveSession, !active());
        self.wake();
    }
    pub(crate) fn lifecycle(&self) -> Lifecycle {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .lifecycle
    }
    pub(crate) fn permits_delivery(
        &self,
        session: Uuid,
        epoch: u64,
        revision: u64,
        generation: u64,
    ) -> bool {
        let s = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        s.lifecycle.epoch == epoch
            && !s.lifecycle.blocked()
            && s.editor.is_none()
            && s.control_revision == revision
            && s.target_generation == generation
            && s.stop != Some((session, true))
            && s.cancelled_session != Some(session)
    }
    /// Linearize opening a capture / accepting a result against native
    /// interruption. The closure must not call back into NativeInput.
    pub(super) fn while_current<T>(
        &self,
        epoch: u64,
        control_revision: u64,
        action: impl FnOnce() -> T,
    ) -> Option<T> {
        let state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.lifecycle.epoch != epoch
            || state.lifecycle.blocked()
            || state.editor.is_some()
            || state.control_revision != control_revision
        {
            return None;
        }
        Some(action())
    }
    /// Accept one refinement result against the sticky native cancel signal.
    /// The closure must only update actor-owned data and never call NativeInput.
    pub(super) fn accept_refinement<T>(
        &self,
        session: Uuid,
        epoch: u64,
        revision: u64,
        action: impl FnOnce() -> T,
    ) -> Option<T> {
        let state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.lifecycle.epoch != epoch
            || state.lifecycle.blocked()
            || state.editor.is_some()
            || state.control_revision != revision
            || state.stop == Some((session, true))
            || state.cancelled_session == Some(session)
        {
            return None;
        }
        Some(action())
    }
    pub(crate) fn acknowledge_quiesced(&self, epoch: u64) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.quiesced_epoch = state.quiesced_epoch.max(epoch);
        self.0.quiescence.notify_all();
    }
    /// Called synchronously by NSWorkspace's will-sleep observer. It never
    /// waits for WebView rendering, database work or model verification.
    pub(crate) fn wait_quiesced(&self, epoch: u64, timeout: Duration) -> bool {
        let state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        let (state, _) = self
            .0
            .quiescence
            .wait_timeout_while(state, timeout, |s| s.quiesced_epoch < epoch)
            .unwrap_or_else(|e| e.into_inner());
        state.quiesced_epoch >= epoch
    }
    pub(super) fn stop(&self, session: Uuid, revision: u64, cancel: bool) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if revision < state.stop_revision {
            return;
        }
        state.stop_revision = revision;
        if cancel {
            state.cancelled_session = Some(session);
        }
        // Cancellation for the same session is never downgraded by a later stop.
        if state.stop != Some((session, true)) {
            state.stop = Some((session, cancel));
        }
        self.wake();
    }
    pub(super) fn take(&self) -> InputSnapshot {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.expire_editor();
        InputSnapshot {
            press: state.down.clone(),
            toggle: state.toggle.take(),
            stop: state.stop.take(),
            lifecycle: state.lifecycle,
            require_release: state.require_release && !state.hold_disabled,
            editing: state.editor.is_some(),
        }
    }
}
fn edit_expired() -> speakmore_core::error::AppError {
    speakmore_core::error::AppError::new(
        "ShortcutEditExpired",
        "编辑已结束，请重新打开快捷键设置。",
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_app_is_frozen_per_press_and_stale_initialization_cannot_replace_activation() {
        let input = NativeInput::default();
        input.application_activated(Some("ORG.TEST.PRIVATE".into()));
        input.key(true);
        let first = input.take().press.unwrap();
        input.application_activated(Some("org.test.public".into()));
        input.initialize_application(1, Some("org.test.stale".into()));
        let held = input.take().press.unwrap();
        assert_eq!(held.app_identity.as_deref(), Some("org.test.private"));
        assert_eq!(held.cycle, first.cycle);
        input.key(false);
        input.key(true);
        let next = input.take().press.unwrap();
        assert_eq!(next.app_identity.as_deref(), Some("org.test.public"));
        input.key(false);
        input.application_activated(Some("/Applications/private.app".into()));
        input.key(true);
        assert!(input.take().press.unwrap().app_identity.is_none());
    }
    #[test]
    fn delivery_guard_remembers_cancel_after_latch_drain_and_checks_focus_generation() {
        let input = NativeInput::default();
        let id = Uuid::new_v4();
        assert!(input.permits_delivery(id, 0, 0, 0));
        input.cancel_key(true, Some(id), 1);
        assert_eq!(input.take().stop, Some((id, true)));
        assert!(!input.permits_delivery(id, 0, 0, 0));
        let next = Uuid::new_v4();
        assert!(input.permits_delivery(next, 0, 0, 0));
        input.target_changed();
        assert!(!input.permits_delivery(next, 0, 0, 0));
        assert!(input.permits_delivery(next, 0, 0, 1));
        input.interrupt(Interruption::DisplaySleep, true);
        assert!(!input.permits_delivery(next, 0, 0, 1));
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn native_capture_is_bound_to_the_live_noncommitting_edit_lease() {
        let input = NativeInput::default();
        let id = input.begin_edit().unwrap();
        assert!(input.edit_matches(id));
        assert!(!input.edit_matches(Uuid::new_v4()));
        input.commit_edit(id).unwrap();
        assert!(!input.edit_matches(id));
        input.release_commit(id);
        assert!(input.edit_matches(id));
        input.end_edit(id, false);
        assert!(!input.edit_matches(id));
        let next = input.begin_edit().unwrap();
        assert!(!input.edit_matches(id));
        assert!(input.edit_matches(next));
        input.interrupt(Interruption::SystemSleep, true);
        assert!(!input.edit_matches(next));
    }
    #[test]
    fn editor_blocks_recording_and_stale_leases_cannot_unlock_a_commit() {
        let input = NativeInput::default();
        let first = input.begin_edit().unwrap();
        input.key(true);
        assert!(input.take().press.is_none());
        input.end_edit(first, false);
        input.key(true); // an observed held key still needs a release
        assert!(input.take().press.is_none());
        input.key(false);
        let second = input.begin_edit().unwrap();
        input.end_edit(first, false);
        assert!(input.editing());
        input.commit_edit(second).unwrap();
        input.end_edit(second, false);
        input
            .0
            .state
            .lock()
            .unwrap()
            .editor
            .as_mut()
            .unwrap()
            .expires = Instant::now() - Duration::from_secs(1);
        assert!(input.editing()); // a pending durable save owns its guard
        input.release_commit(second);
        input.end_edit(second, false);
        input.key(true); // a fresh physical press works after an ordinary edit
        assert!(input.take().press.is_some());
        input.key(false);
        let third = input.begin_edit().unwrap();
        input
            .0
            .state
            .lock()
            .unwrap()
            .editor
            .as_mut()
            .unwrap()
            .expires = Instant::now() - Duration::from_secs(1);
        assert!(input.renew_edit(third).is_err());
        assert!(!input.editing());
    }
    #[test]
    fn held_cancel_cannot_cancel_a_later_session_without_release() {
        let input = NativeInput::default();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        input.cancel_key(true, Some(first), 1);
        assert_eq!(input.take().stop, Some((first, true)));
        for _ in 0..1000 {
            input.cancel_key(true, Some(second), 2);
        }
        assert!(input.take().stop.is_none());
        input.cancel_key(false, Some(first), 1);
        input.cancel_key(true, Some(second), 2);
        assert_eq!(input.take().stop, Some((second, true)));
    }
    #[test]
    fn overlapping_interruptions_and_quick_resume_do_not_replay_a_held_key() {
        let input = NativeInput::default();
        input.key(true);
        let before = input.lifecycle().epoch;
        input.interrupt(Interruption::DisplaySleep, true);
        let epoch = input.interrupt(Interruption::SystemSleep, true);
        input.interrupt(Interruption::DisplaySleep, true); // duplicate
        assert_eq!(input.lifecycle().epoch, epoch);
        input.interrupt(Interruption::SystemSleep, false);
        assert!(input.lifecycle().blocked());
        input.interrupt(Interruption::DisplaySleep, false);
        assert!(!input.lifecycle().blocked());
        assert!(epoch > before);
        assert!(input
            .while_current(before, 0, || panic!("stale admission"))
            .is_none());
        for _ in 0..1000 {
            input.key(true);
        }
        assert!(input.take().press.is_none());
        input.key(false);
        input.key(true);
        assert!(input.take().press.is_some());
    }
    #[test]
    fn only_display_sleep_keeps_the_worker_across_an_interruption() {
        let input = NativeInput::default();
        let start = input.lifecycle().epoch;
        input.interrupt(Interruption::DisplaySleep, true);
        let display = input.lifecycle().epoch;
        assert!(display > start && !input.lifecycle().teardown_after(start));
        input.interrupt(Interruption::DisplaySleep, false);
        input.interrupt(Interruption::SystemSleep, true);
        input.interrupt(Interruption::SystemSleep, false); // between actor polls
        assert!(input.lifecycle().teardown_after(display));
        let resumed = input.lifecycle().epoch;
        input.interrupt(Interruption::InactiveSession, true);
        assert!(input.lifecycle().teardown_after(resumed));
        input.interrupt(Interruption::InactiveSession, false);
        let active = input.lifecycle().epoch;
        input.permissions_changed(true);
        assert!(input.lifecycle().teardown_after(active));
    }
    #[test]
    fn quiescence_acknowledgement_cannot_satisfy_a_new_interruption() {
        let input = NativeInput::default();
        let first = input.interrupt(Interruption::DisplaySleep, true);
        input.acknowledge_quiesced(first);
        assert!(input.wait_quiesced(first, Duration::ZERO));
        let second = input.interrupt(Interruption::SystemSleep, true);
        assert!(!input.wait_quiesced(second, Duration::ZERO));
        let ack = input.clone();
        let thread = std::thread::spawn(move || {
            ack.acknowledge_quiesced(second);
        });
        assert!(input.wait_quiesced(second, Duration::from_secs(1)));
        thread.join().unwrap();
    }
    #[test]
    fn coalesces_repeats_and_completed_taps_but_preserves_new_cycles() {
        let input = NativeInput::default();
        input.key(true);
        let first = input.take().press.unwrap().cycle;
        for _ in 0..10_000 {
            input.key(true);
        }
        assert_eq!(input.take().press.unwrap().cycle, first);
        input.key(false);
        input.key(true);
        assert_ne!(input.take().press.unwrap().cycle, first);
        input.key(false);
        assert!(input.take().press.is_none());
        input.key(true);
        input.key(false);
        assert!(input.take().press.is_none());
    }
    #[test]
    fn native_cancel_keeps_identity_and_dominates_stop() {
        let input = NativeInput::default();
        let id = Uuid::new_v4();
        input.stop(id, 2, true);
        input.stop(id, 2, false);
        input.stop(Uuid::new_v4(), 1, false);
        assert_eq!(input.take().stop, Some((id, true)));
        assert_eq!(input.take().stop, None);
    }
}
