use super::*;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
#[derive(Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub name: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub revision: String,
    pub files: Vec<Artifact>,
}
pub fn models() -> Vec<Model> {
    serde_json::from_str(include_str!("../../../resources/meeting-models.lock.json"))
        .expect("fixed model catalog")
}
pub fn root(s: &Service, id: &str) -> PathBuf {
    s.0.text_root.parent().unwrap().join("meeting-asr").join(id)
}
pub fn ready(s: &Service, id: &str) -> bool {
    models().iter().find(|m| m.id == id).is_some_and(|m| {
        m.files.iter().all(|f| {
            std::fs::metadata(root(s, id).join(&f.name)).is_ok_and(|meta| meta.len() == f.size)
        })
    })
}
pub fn verify(s: &Service, id: &str, cancel: &AtomicBool) -> Result<()> {
    let model = models()
        .into_iter()
        .find(|m| m.id == id)
        .ok_or_else(|| AppError::invalid("模型不存在。"))?;
    for a in model.files {
        check_cancel(cancel)?;
        let mut f = std::fs::File::open(root(s, id).join(&a.name))
            .map_err(|_| err("ModelRequired", "请先在语音模型中下载所选模型。"))?;
        let mut hash = Sha256::new();
        let mut bytes = [0; 65536];
        loop {
            check_cancel(cancel)?;
            let n = f.read(&mut bytes).map_err(store::io)?;
            if n == 0 {
                break;
            }
            hash.update(&bytes[..n]);
        }
        if format!("{:x}", hash.finalize()) != a.sha256 {
            return Err(err("ModelInvalid", "模型完整性校验失败，请重新下载。"));
        }
    }
    Ok(())
}
#[tauri::command]
pub fn meeting_models(
    window: WebviewWindow,
    state: State<'_, Managed>,
) -> Result<serde_json::Value> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    let active =
        s.0.download
            .lock()
            .unwrap()
            .as_ref()
            .map(|a| a.kind.clone());
    let download_model = s.0.download_model.lock().unwrap().clone();
    let phase = s.0.download_phase.lock().unwrap().clone();
    let error = s.0.download_error.lock().unwrap().clone();
    Ok(serde_json::json!({
        "models": models().iter().map(|m| serde_json::json!({
            "id":m.id, "name":m.name, "size":m.files.iter().map(|f|f.size).sum::<u64>(), "ready":ready(&s,&m.id)
        })).collect::<Vec<_>>(),
        "downloaded":s.0.downloaded.load(Ordering::Acquire), "error":error,
        "active":active, "busy":s.busy(), "mossRuntime":moss_runtime::status(&s),
        "downloadModel":download_model, "downloadPhase":phase,
    }))
}
#[tauri::command]
pub async fn meeting_model_download(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: String,
) -> Result<()> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    let model = models()
        .into_iter()
        .find(|m| m.id == id)
        .ok_or_else(|| AppError::invalid("模型不存在。"))?;
    let guard = s.begin(None, "download").await?;
    *s.0.download_model.lock().unwrap() = Some(id);
    s.0.downloaded.store(0, Ordering::Release);
    *s.0.download_error.lock().unwrap() = None;
    tauri::async_runtime::spawn(async move {
        let result = async {
            if model.id == "moss" {
                *s.0.download_phase.lock().unwrap() = Some("downloading_runtime".into());
                moss_runtime::ensure(&s, guard.active.cancel.clone()).await?;
            }
            *s.0.download_phase.lock().unwrap() = Some("downloading_model".into());
            s.0.downloaded.store(0, Ordering::Release);
            download(&s, &model, &guard.active.cancel).await?;
            if model.id == "moss" {
                *s.0.download_model.lock().unwrap() = Some("speaker".into());
                s.0.downloaded.store(0, Ordering::Release);
                s.changed();
                let speaker = models()
                    .into_iter()
                    .find(|m| m.id == "speaker")
                    .ok_or_else(|| store::io("speaker catalog"))?;
                download(&s, &speaker, &guard.active.cancel).await?;
            }
            Ok::<(), AppError>(())
        }
        .await;
        if let Err(e) = result {
            *s.0.download_error.lock().unwrap() = Some(e.user_message);
        }
        drop(guard);
    });
    Ok(())
}
async fn download(s: &Service, m: &Model, cancel: &AtomicBool) -> Result<()> {
    let dir = root(s, &m.id);
    std::fs::create_dir_all(&dir).map_err(store::io)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .https_only(true)
        .connect_timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::custom(|a| {
            let h = a.url().host_str().unwrap_or("");
            if a.previous().len() < 6
                && a.url().scheme() == "https"
                && (h == "huggingface.co"
                    || h.ends_with(".huggingface.co")
                    || h == "hf.co"
                    || h.ends_with(".hf.co"))
            {
                a.follow()
            } else {
                a.stop()
            }
        }))
        .build()
        .map_err(store::io)?;
    let mut total = 0;
    for a in &m.files {
        check_cancel(cancel)?;
        let path = dir.join(&a.name);
        if path.exists() {
            let mut f = std::fs::File::open(&path).map_err(store::io)?;
            let mut h = Sha256::new();
            let mut buf = [0; 65536];
            loop {
                check_cancel(cancel)?;
                let n = f.read(&mut buf).map_err(store::io)?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
            }
            if format!("{:x}", h.finalize()) == a.sha256 {
                total += a.size;
                s.0.downloaded.store(total, Ordering::Release);
                continue;
            }
        }
        let mut response = client
            .get(&a.url)
            .send()
            .await
            .map_err(|_| err("ModelNetwork", "模型下载连接失败。"))?
            .error_for_status()
            .map_err(|_| err("ModelNetwork", "模型下载服务不可用。"))?;
        let mut f = tempfile::NamedTempFile::new_in(&dir).map_err(store::io)?;
        let mut hash = Sha256::new();
        let mut count = 0;
        loop {
            check_cancel(cancel)?;
            let chunk = tokio::time::timeout(Duration::from_secs(30), response.chunk())
                .await
                .map_err(|_| err("ModelTimeout", "下载超时，可重试继续已完成的文件。"))?
                .map_err(store::io)?;
            let Some(chunk) = chunk else {
                break;
            };
            count += chunk.len() as u64;
            if count > a.size {
                return Err(store::io("model bound"));
            }
            hash.update(&chunk);
            f.write_all(&chunk).map_err(store::io)?;
            s.0.downloaded.store(total + count, Ordering::Release);
        }
        if count != a.size || format!("{:x}", hash.finalize()) != a.sha256 {
            return Err(err("ModelInvalid", "下载文件校验失败。"));
        }
        f.as_file().sync_all().map_err(store::io)?;
        f.persist(path).map_err(store::io)?;
        total += count;
    }
    Ok(())
}
#[tauri::command]
pub async fn meeting_model_remove(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: String,
) -> Result<()> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    let _guard = s.begin(None, "removing_model").await?;
    if !models().iter().any(|m| m.id == id) {
        return Err(AppError::invalid("模型不存在。"));
    }
    let dir = root(&s, &id);
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(store::io)?;
    }
    Ok(())
}
