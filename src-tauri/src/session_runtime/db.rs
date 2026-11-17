use super::{actor::Actor, SharedStore};
use speakmore_core::{
    error::{AppError, Result},
    storage::Store,
    types::{DictionaryEntry, Settings},
};
use std::sync::mpsc::{self, SyncSender};
use tokio::sync::mpsc::Sender;

#[derive(Clone)]
pub(super) struct View {
    pub settings: Settings,
    pub writing_rules: Vec<speakmore_core::writing::Rule>,
    pub dictionary: Vec<DictionaryEntry>,
    pub terms: Vec<String>,
    pub usage_today: u32,
    pub quota_day: String,
    pub history_guard: uuid::Uuid,
    pub app_policies: std::sync::Arc<speakmore_core::app_rules::Policies>,
    pub provider: Option<std::sync::Arc<speakmore_core::providers::ProviderRecord>>,
    pub processing: std::sync::Arc<speakmore_core::processing::Policies>,
}
impl View {
    fn read(store: &Store) -> Result<Self> {
        let settings = store.settings()?;
        let (_, terms) = store.dictionary_snapshot(&settings.language)?;
        let quota_day = chrono::Local::now().format("%Y-%m-%d").to_string();
        Ok(Self {
            provider: if settings.polish_enabled {
                store.providers().ok().and_then(|rows| {
                    rows.into_iter()
                        .find(|r| {
                            Some(r.draft.provider_id.to_string()) == settings.active_provider_id
                        })
                        .map(std::sync::Arc::new)
                })
            } else {
                None
            },
            processing: store.processing_policies(),
            writing_rules: store.writing_rules()?,
            dictionary: store.dictionary()?,
            history_guard: store.history_guard(),
            app_policies: store.app_policies(),
            terms,
            usage_today: store.usage_today(&quota_day)?,
            quota_day,
            settings,
        })
    }
}
pub(super) type Applied = Box<dyn FnOnce(&mut Actor) + Send>;
pub(super) type Task = Box<dyn FnOnce(Result<&mut Store>) -> Applied + Send>;
/// A task plus whether it may change the published view. Read-only tasks
/// skip the view queries and never report configuration activity.
pub(super) type Job = (Task, bool);
pub(super) struct Finished {
    pub view: Option<Result<View>>,
    pub applied: Applied,
}
pub(super) fn spawn(
    store: SharedStore,
    completed: Sender<super::actor::Completed>,
) -> Result<SyncSender<Job>> {
    let (sender, receiver) = mpsc::sync_channel::<Job>(32);
    std::thread::Builder::new()
        .name("speakmore-storage".into())
        .spawn(move || {
            while let Ok((task, refresh)) = receiver.recv() {
                let (applied, view) = match store.lock() {
                    Ok(mut guard) => match guard.as_mut() {
                        Ok(store) => {
                            let applied = task(Ok(store));
                            (applied, refresh.then(|| View::read(store)))
                        }
                        Err(error) => (
                            task(Err(error.clone())),
                            refresh.then(|| Err(error.clone())),
                        ),
                    },
                    Err(_) => (
                        task(Err(unavailable())),
                        refresh.then(|| Err(unavailable())),
                    ),
                };
                // Never hold the connection mutex while waiting for the actor mailbox.
                if completed
                    .blocking_send(super::actor::Completed::Database(Finished {
                        view,
                        applied,
                    }))
                    .is_err()
                {
                    break;
                }
            }
        })
        .map_err(|_| unavailable())?;
    Ok(sender)
}
pub(super) fn unavailable() -> AppError {
    AppError::new(
        "StorageUnavailable",
        "本地数据服务不可用，请重新启动。",
        true,
    )
}
