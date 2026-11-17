//! Optional Python runtime. The app only ships the small, pinned MOSS source package.
use super::*;
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    io::{Read, Write},
    process::{Command, Stdio},
};

const MANIFEST: &str = include_str!("../../../resources/moss-runtime-downloads.lock.json");
#[derive(Deserialize)]
struct Manifest {
    version: String,
    python: catalog::Artifact,
    wheels: Vec<catalog::Artifact>,
}
fn manifest() -> Manifest {
    serde_json::from_str(MANIFEST).expect("pinned MOSS runtime")
}
fn fingerprint() -> String {
    format!("{:x}", Sha256::digest(MANIFEST.as_bytes()))
}
fn base(s: &Service) -> PathBuf {
    s.0.root.parent().unwrap().join("runtimes/moss")
}
fn installed_dir(base: &Path) -> PathBuf {
    base.join(manifest().version)
}
fn is_ready(base: &Path) -> bool {
    let dir = installed_dir(base);
    std::fs::read_to_string(dir.join(".speakmore-ready")).is_ok_and(|v| v == fingerprint())
        && dir.join("bin/python3.12").is_file()
        && dir
            .join("lib/python3.12/site-packages/torch/__init__.py")
            .is_file()
        && dir
            .join("lib/python3.12/site-packages/transformers/__init__.py")
            .is_file()
}
pub fn ready(s: &Service) -> bool {
    is_ready(&base(s))
}
pub fn python(s: &Service) -> Result<PathBuf> {
    if !ready(s) {
        return Err(err(
            "MossRuntimeRequired",
            "请在设置 → 语音模型中安装 MOSS 运行环境，再继续整理。",
        ));
    }
    Ok(installed_dir(&base(s)).join("bin/python3.12"))
}
pub fn status(s: &Service) -> serde_json::Value {
    let m = manifest();
    serde_json::json!({"ready":ready(s), "downloadBytes":m.python.size + m.wheels.iter().map(|a|a.size).sum::<u64>(), "path":base(s)})
}
fn allowed_host(host: &str) -> bool {
    matches!(
        host,
        "github.com"
            | "release-assets.githubusercontent.com"
            | "objects.githubusercontent.com"
            | "files.pythonhosted.org"
    )
}
async fn cancellable<F: Future>(future: F, cancel: &AtomicBool) -> Result<F::Output> {
    tokio::pin!(future);
    loop {
        check_cancel(cancel)?;
        tokio::select! {
            value = &mut future => return Ok(value),
            _ = tokio::time::sleep(Duration::from_millis(100)) => {},
        }
    }
}
fn verified(path: &Path, a: &catalog::Artifact, cancel: &AtomicBool) -> Result<bool> {
    if !std::fs::metadata(path).is_ok_and(|m| m.len() == a.size) {
        return Ok(false);
    }
    let mut file = std::fs::File::open(path).map_err(store::io)?;
    let mut hash = Sha256::new();
    let mut buf = [0; 65536];
    loop {
        check_cancel(cancel)?;
        let n = file.read(&mut buf).map_err(store::io)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()) == a.sha256)
}
async fn download(base: &Path, progress: &AtomicU64, cancel: &AtomicBool) -> Result<PathBuf> {
    let m = manifest();
    let cache = base.join("downloads");
    std::fs::create_dir_all(&cache).map_err(store::io)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .https_only(true)
        .connect_timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::custom(|a| {
            if a.previous().len() < 6
                && a.url().scheme() == "https"
                && allowed_host(a.url().host_str().unwrap_or(""))
            {
                a.follow()
            } else {
                a.stop()
            }
        }))
        .build()
        .map_err(store::io)?;
    let mut total = 0;
    for a in std::iter::once(&m.python).chain(&m.wheels) {
        check_cancel(cancel)?;
        let url = url::Url::parse(&a.url).map_err(store::io)?;
        if url.scheme() != "https"
            || !allowed_host(url.host_str().unwrap_or(""))
            || Path::new(&a.name).components().count() != 1
        {
            return Err(store::io("invalid runtime artifact"));
        }
        let dest = cache.join(&a.name);
        if verified(&dest, a, cancel)? {
            total += a.size;
            progress.store(total, Ordering::Release);
            continue;
        }
        let mut response = cancellable(client.get(url).send(), cancel)
            .await?
            .map_err(|_| err("RuntimeNetwork", "MOSS 运行环境下载连接失败，可重试。"))?
            .error_for_status()
            .map_err(|_| err("RuntimeNetwork", "MOSS 运行环境下载服务不可用，可重试。"))?;
        let mut file = tempfile::NamedTempFile::new_in(&cache).map_err(store::io)?;
        let mut hash = Sha256::new();
        let mut count = 0;
        loop {
            let chunk = cancellable(
                tokio::time::timeout(Duration::from_secs(30), response.chunk()),
                cancel,
            )
            .await?
            .map_err(|_| {
                err(
                    "RuntimeTimeout",
                    "运行环境下载超时，可重试继续已完成的文件。",
                )
            })?
            .map_err(store::io)?;
            let Some(chunk) = chunk else { break };
            count += chunk.len() as u64;
            if count > a.size {
                return Err(err("RuntimeInvalid", "运行环境文件大小不符，请重试。"));
            }
            hash.update(&chunk);
            file.write_all(&chunk).map_err(store::io)?;
            progress.store(total + count, Ordering::Release);
        }
        if count != a.size || format!("{:x}", hash.finalize()) != a.sha256 {
            return Err(err("RuntimeInvalid", "运行环境文件校验失败，请重试。"));
        }
        file.as_file().sync_all().map_err(store::io)?;
        file.persist(dest).map_err(store::io)?;
        total += count;
    }
    Ok(cache)
}
fn run(command: &mut Command, cancel: &AtomicBool) -> Result<()> {
    check_cancel(cancel)?;
    let mut child = command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| err("RuntimeInstall", "无法启动 MOSS 环境安装，请重试。"))?;
    let deadline = Instant::now() + Duration::from_secs(600);
    let result = loop {
        if let Err(error) = check_cancel(cancel) {
            break Err(error);
        }
        if Instant::now() >= deadline {
            break Err(err("RuntimeInstall", "MOSS 环境安装超时，请重试。"));
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break Ok(()),
            Ok(Some(_)) => break Err(err("RuntimeInstall", "MOSS 环境安装或检查失败，请重试。")),
            Err(_) => break Err(err("RuntimeInstall", "无法检查 MOSS 安装进度，请重试。")),
            _ => std::thread::sleep(Duration::from_millis(100)),
        }
    };
    if result.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    result
}
fn unpack_and_install(
    base: &Path,
    workers: &Path,
    cache: &Path,
    cancel: &AtomicBool,
) -> Result<()> {
    let m = manifest();
    // An application crash cannot run TempDir cleanup. Reclaim only our own stale stages.
    for entry in std::fs::read_dir(base).map_err(store::io)? {
        let entry = entry.map_err(store::io)?;
        if entry.file_name().to_string_lossy().starts_with("install-")
            && entry.file_type().map_err(store::io)?.is_dir()
        {
            std::fs::remove_dir_all(entry.path()).map_err(store::io)?;
        }
    }
    // A staged install is never advertised as ready; failures/cancellation remove it.
    let staging = tempfile::Builder::new()
        .prefix("install-")
        .tempdir_in(base)
        .map_err(store::io)?;
    run(
        Command::new("/usr/bin/tar")
            .arg("-xzf")
            .arg(cache.join(&m.python.name))
            .arg("-C")
            .arg(staging.path()),
        cancel,
    )?;
    let python_root = staging.path().join("python");
    let python = python_root.join("bin/python3.12");
    let mut pip = Command::new(&python);
    pip.args([
        "-I",
        "-m",
        "pip",
        "--isolated",
        "install",
        "--no-index",
        "--no-deps",
        "--no-compile",
        "--disable-pip-version-check",
        "--no-warn-script-location",
        "--target",
    ])
    .arg(python_root.join("lib/python3.12/site-packages"));
    for a in &m.wheels {
        pip.arg(cache.join(&a.name));
    }
    run(&mut pip, cancel)?;
    // Imports exercise the native wheels without loading weights or sending data.
    run(Command::new(&python).args(["-I", "-c", "import sys; sys.path.insert(0, sys.argv[1]); import torch, torchaudio; from moss_transcribe_diarize import MossTranscribeDiarizeForConditionalGeneration, MossTranscribeDiarizeProcessor, parse_transcript; from moss_transcribe_diarize.inference_utils import build_transcription_messages, generate_transcription"])
        .arg(workers.join("moss-package")), cancel)?;
    check_cancel(cancel)?;
    std::fs::write(python_root.join(".speakmore-ready"), fingerprint()).map_err(store::io)?;
    let destination = installed_dir(base);
    if destination.exists() {
        std::fs::remove_dir_all(&destination).map_err(store::io)?;
    }
    std::fs::rename(&python_root, destination).map_err(store::io)?;
    // Verified wheels are retained only after an interrupted installation for retry.
    let _ = std::fs::remove_dir_all(cache);
    Ok(())
}
pub async fn ensure(s: &Service, cancel: Arc<AtomicBool>) -> Result<()> {
    if ready(s) {
        return Ok(());
    }
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return Err(err(
            "RuntimePlatform",
            "MOSS 运行环境目前支持 Apple 芯片的 Mac。",
        ));
    }
    let dir = base(s);
    let cache = download(&dir, &s.0.downloaded, &cancel).await?;
    *s.0.download_phase.lock().unwrap() = Some("installing_runtime".into());
    let workers = s.0.workers.clone();
    tauri::async_runtime::spawn_blocking(move || {
        unpack_and_install(&dir, &workers, &cache, &cancel)
    })
    .await
    .map_err(store::io)?
}
#[tauri::command]
pub async fn meeting_moss_runtime_download(
    window: WebviewWindow,
    state: State<'_, Managed>,
) -> Result<()> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    let guard = s.begin(None, "download").await?;
    *s.0.download_model.lock().unwrap() = Some("moss-runtime".into());
    *s.0.download_phase.lock().unwrap() = Some("downloading_runtime".into());
    s.0.downloaded.store(0, Ordering::Release);
    *s.0.download_error.lock().unwrap() = None;
    tauri::async_runtime::spawn(async move {
        if let Err(e) = ensure(&s, guard.active.cancel.clone()).await {
            *s.0.download_error.lock().unwrap() = Some(e.user_message);
        }
        drop(guard);
    });
    Ok(())
}
#[tauri::command]
pub async fn meeting_moss_runtime_remove(
    window: WebviewWindow,
    state: State<'_, Managed>,
) -> Result<()> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    let _guard = s.begin(None, "removing_model").await?;
    let dir = base(&s);
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(store::io)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incomplete_or_old_install_is_not_ready() {
        let base = tempfile::tempdir().unwrap();
        let dir = installed_dir(base.path());
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/python3.12"), "partial").unwrap();
        assert!(!is_ready(base.path()));
        std::fs::write(dir.join(".speakmore-ready"), "old").unwrap();
        assert!(!is_ready(base.path()));
    }
    #[test]
    fn corrupt_cached_artifact_is_not_reused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wheel");
        let a = catalog::Artifact {
            name: "wheel".into(),
            url: "".into(),
            size: 4,
            sha256: format!("{:x}", Sha256::digest(b"good")),
        };
        std::fs::write(&path, "evil").unwrap();
        let cancel = AtomicBool::new(false);
        assert!(!verified(&path, &a, &cancel).unwrap());
        std::fs::write(&path, "good").unwrap();
        assert!(verified(&path, &a, &cancel).unwrap());
        cancel.store(true, Ordering::Release);
        assert!(verified(&path, &a, &cancel).is_err());
    }
    #[tokio::test]
    async fn cancellation_interrupts_a_pending_download() {
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            c.store(true, Ordering::Release);
        });
        assert!(cancellable(std::future::pending::<()>(), &cancel)
            .await
            .is_err());
    }
    #[tokio::test]
    #[ignore = "downloads pinned Python and wheels; isolated project cache only"]
    async fn real_install_without_bundled_python() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let base = root.join(".cache/moss-on-demand-check");
        let cancel = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        if !is_ready(&base) {
            let cache = download(&base, &progress, &cancel).await.unwrap();
            unpack_and_install(&base, &root.join("resources/workers"), &cache, &cancel).unwrap();
        }
        assert!(is_ready(&base));
        assert!(!base.join("downloads").exists());
    }
}
