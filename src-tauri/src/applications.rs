//! User-picked app metadata only. No arbitrary WebView paths and no executable loading.
#[cfg(target_os = "macos")]
pub(crate) mod mac;
use speakmore_core::{
    error::{AppError, Result},
    types::AppRule,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Update {
    pub expected_revision: i64,
    pub selection_id: Option<Uuid>,
    pub app_identity: Option<String>,
    pub manual_only: bool,
    pub history_excluded: bool,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    pub revision: i64,
    pub items: Vec<AppRule>,
}
pub(crate) fn snapshot(store: &speakmore_core::storage::Store) -> Result<Snapshot> {
    Ok(Snapshot {
        revision: store.settings()?.revision,
        items: store
            .app_rules()?
            .into_iter()
            .filter(|r| r.platform == "macos")
            .collect(),
    })
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Choice {
    pub selection_id: Uuid,
    pub app_identity: String,
    pub display_name: String,
}
#[derive(Default)]
pub(crate) struct Selections(Mutex<Option<(Instant, Choice)>>);
impl Selections {
    pub(crate) fn put(&self, identity: String, name: String) -> Choice {
        let choice = Choice {
            selection_id: Uuid::new_v4(),
            app_identity: identity,
            display_name: name,
        };
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), choice.clone()));
        choice
    }
    pub(crate) fn resolve(&self, id: Uuid) -> Result<Choice> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state
            .as_ref()
            .is_some_and(|(at, _)| at.elapsed() >= Duration::from_secs(300))
        {
            state.take();
        }
        state
            .as_ref()
            .filter(|(_, c)| c.selection_id == id)
            .map(|(_, c)| c.clone())
            .ok_or_else(|| {
                AppError::new("AppSelectionExpired", "应用选择已失效，请重新选择。", true)
            })
    }
    pub(crate) fn consume(&self, id: Uuid) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.as_ref().is_some_and(|(_, c)| c.selection_id == id) {
            state.take();
        }
    }
}
pub(crate) fn selected_rule(choice: Choice, manual: bool, excluded: bool) -> AppRule {
    AppRule {
        platform: "macos".into(),
        app_identity: choice.app_identity,
        display_name: choice.display_name,
        manual_only: manual,
        history_excluded: excluded,
    }
}
static CHOOSING: AtomicBool = AtomicBool::new(false);
pub(crate) struct Guard;
impl Guard {
    pub(crate) fn acquire() -> Result<Self> {
        CHOOSING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| {
                AppError::new(
                    "AppPickerBusy",
                    "请先完成或取消已打开的文件选择窗口。",
                    true,
                )
            })
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        CHOOSING.store(false, Ordering::Release);
    }
}
pub(crate) async fn choose(window: &tauri::WebviewWindow) -> Result<Option<(String, String)>> {
    #[cfg(target_os = "macos")]
    {
        mac::choose(window, Guard::acquire()?).await
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        Err(AppError::new(
            "UnsupportedPlatform",
            "当前仅在 macOS 提供应用选择。",
            false,
        ))
    }
}
fn picker_error() -> AppError {
    AppError::new(
        "AppPickerUnavailable",
        "无法读取所选应用，请选择有效的 .app 应用。",
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_the_current_unexpired_native_selection_can_create_a_rule() {
        let selections = Selections::default();
        let first = selections.put("org.test.first".into(), "First".into());
        assert!(selections.resolve(Uuid::new_v4()).is_err());
        let second = selections.put("org.test.second".into(), "Second".into());
        assert!(selections.resolve(first.selection_id).is_err());
        selections.consume(first.selection_id);
        assert_eq!(
            selections
                .resolve(second.selection_id)
                .unwrap()
                .app_identity,
            "org.test.second"
        );
        selections.0.lock().unwrap().as_mut().unwrap().0 =
            Instant::now() - Duration::from_secs(301);
        assert!(selections.resolve(second.selection_id).is_err());
        assert!(selections.0.lock().unwrap().is_none());
        let third = selections.put("org.test.third".into(), "Third".into());
        selections.consume(third.selection_id);
        assert!(selections.resolve(third.selection_id).is_err());
    }
}
