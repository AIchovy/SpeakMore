//! App restrictions are latched for each live session. Relaxing or deleting a
//! rule affects the next session; a database round trip cannot restore consent.
use crate::{
    error::{AppError, Result},
    types::AppRule,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, Weak,
};

pub const MAX_RULES: usize = 100;
pub fn macos_identity(value: &str) -> Result<String> {
    if value.is_empty()
        || value.len() > 255
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
    {
        return Err(AppError::invalid("应用标识无效，请重新选择应用。"));
    }
    Ok(value.to_ascii_lowercase())
}
pub fn validate(rule: &AppRule) -> Result<()> {
    if rule.platform != "macos"
        || macos_identity(&rule.app_identity)? != rule.app_identity
        || rule.display_name.trim().is_empty()
        || rule.display_name.len() > 512
        || rule.display_name.chars().any(char::is_control)
    {
        return Err(AppError::invalid("应用规则无效，请重新选择应用。"));
    }
    Ok(())
}

pub struct Watch {
    identity: Option<String>,
    manual: AtomicBool,
    excluded: AtomicBool,
}
impl Watch {
    pub fn manual_only(&self) -> bool {
        self.manual.load(Ordering::Acquire)
    }
    pub fn history_excluded(&self) -> bool {
        self.excluded.load(Ordering::Acquire)
    }
}
#[derive(Default)]
struct State {
    rules: Vec<AppRule>,
    watches: Vec<Weak<Watch>>,
}
#[derive(Default)]
pub struct Policies(Mutex<State>);
fn restrictions(rules: &[AppRule], identity: Option<&str>) -> (bool, bool) {
    rules
        .iter()
        .filter(|r| r.platform == "macos" && identity.is_none_or(|id| id == r.app_identity))
        .fold((false, false), |(m, h), r| {
            (m || r.manual_only, h || r.history_excluded)
        })
}
impl Policies {
    pub fn watch(&self, identity: Option<&str>) -> Arc<Watch> {
        let identity = identity.and_then(|id| macos_identity(id).ok());
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let (manual, excluded) = restrictions(&state.rules, identity.as_deref());
        let watch = Arc::new(Watch {
            identity,
            manual: AtomicBool::new(manual),
            excluded: AtomicBool::new(excluded),
        });
        state.watches.retain(|w| w.strong_count() > 0);
        state.watches.push(Arc::downgrade(&watch));
        watch
    }
    pub(crate) fn replace(&self, rules: Vec<AppRule>) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.watches.retain(|weak| {
            let Some(watch) = weak.upgrade() else {
                return false;
            };
            let (manual, excluded) = restrictions(&rules, watch.identity.as_deref());
            if manual {
                watch.manual.store(true, Ordering::Release);
            }
            if excluded {
                watch.excluded.store(true, Ordering::Release);
            }
            true
        });
        state.rules = rules;
    }
}
