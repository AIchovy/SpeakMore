use crate::{catalog::tests::trusted, filesystem as fs, manager::*, ModelError as E};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

#[derive(Default)]
struct RegistryProbe {
    ids: Mutex<Vec<String>>,
    fail: AtomicBool,
}
impl Registry for RegistryProbe {
    fn installed(&self, bundle: &InstalledBundle) -> crate::Result<()> {
        assert!(bundle.directory.join("model.bin").is_file());
        assert!(bundle.directory.join("vad.bin").is_file());
        if self.fail.load(Ordering::SeqCst) {
            return Err(E::Registration);
        }
        self.ids
            .lock()
            .unwrap()
            .push(bundle.model.installation_id());
        Ok(())
    }
    fn removed(&self, model_id: &str) -> crate::Result<()> {
        self.ids.lock().unwrap().retain(|id| id != model_id);
        Ok(())
    }
}
struct Server {
    origin: String,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
type Handler = Arc<dyn Fn(&str) -> Vec<u8> + Send + Sync>;
impl Server {
    fn new(response: Vec<u8>, delay: Duration) -> Self {
        Self::with_handler(Arc::new(move |_| response.clone()), delay)
    }
    /// The handler receives the lowercased request head and returns the raw response.
    fn with_handler(handler: Handler, delay: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        // BSD sockets may inherit the listener's nonblocking
                        // flag. The fixture needs blocking I/O with deadlines.
                        socket.set_nonblocking(false).unwrap();
                        socket
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        socket
                            .set_write_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        // TCP may split request headers. Closing with unread
                        // request bytes can cause RST and a spurious transport
                        // retry instead of the intended HTTP/hash failure.
                        let mut request = Vec::new();
                        let mut chunk = [0; 1024];
                        while request.len() < 8192 && !request.ends_with(b"\r\n\r\n") {
                            match socket.read(&mut chunk) {
                                Ok(0) | Err(_) => break,
                                Ok(n) => request.extend_from_slice(&chunk[..n]),
                            }
                        }
                        seen.fetch_add(1, Ordering::SeqCst);
                        let start = std::time::Instant::now();
                        while start.elapsed() < delay && !stopping.load(Ordering::SeqCst) {
                            thread::sleep(Duration::from_millis(5));
                        }
                        let head = String::from_utf8_lossy(&request).to_ascii_lowercase();
                        let _ = socket.write_all(&handler(&head));
                        let _ = socket.shutdown(std::net::Shutdown::Write);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(e) => panic!("{e}"),
                }
            }
        });
        Self {
            origin,
            requests,
            stop,
            thread: Some(thread),
        }
    }
    fn ok(bytes: &[u8]) -> Self {
        let mut response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        )
        .into_bytes();
        response.extend_from_slice(bytes);
        Self::new(response, Duration::ZERO)
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take().unwrap().join().unwrap();
    }
}
const ID: &str = "whisper-small-q5_1";
fn manager(
    root: std::path::PathBuf,
    bytes: &[u8],
    registry: Arc<RegistryProbe>,
    server: &Server,
) -> ModelManager {
    manager_with_timeout(root, bytes, registry, server, Duration::from_secs(2))
}
fn manager_with_timeout(
    root: PathBuf,
    bytes: &[u8],
    registry: Arc<RegistryProbe>,
    server: &Server,
    read_timeout: Duration,
) -> ModelManager {
    let client = reqwest::Client::builder()
        .no_proxy()
        .retry(reqwest::retry::never())
        .read_timeout(read_timeout)
        .build()
        .unwrap();
    let mut manager =
        ModelManager::open_with_client(root, trusted(bytes), registry, Arc::new(|_| {}), client)
            .unwrap();
    let inner = Arc::get_mut(&mut manager.0).unwrap();
    inner.test_origin = Some(server.origin.clone());
    inner.retry_delays = [Duration::from_millis(10); 2];
    manager
}
async fn settled(manager: &ModelManager) -> ModelsSnapshot {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let state = manager.snapshot();
            if !state.busy {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn catalog_upgrade_preserves_old_install_and_isolates_version_failures_and_removal() {
    use crate::catalog::tests::{fixture, signed};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("models");
    let registry = Arc::new(RegistryProbe::default());
    let original = ModelManager::open(
        root.clone(),
        trusted(b"old"),
        registry.clone(),
        Arc::new(|_| {}),
    )
    .unwrap();
    original.initialize().await.unwrap();
    let sources = import_paths(&temp.path().join("old-source"), b"old");
    original
        .start_import(ID, original.prepare_import(ID, sources).unwrap())
        .unwrap();
    assert_eq!(
        settled(&original).await.models[0].state,
        InstallState::Installed
    );
    let old_path = original.lease(ID).await.unwrap().bundle.directory.clone();
    drop(original);

    let mut payload = fixture(b"old");
    let mut newer = fixture(b"new").models.remove(0);
    newer.version = Some("2026.10".into());
    let next_id = newer.installation_id();
    payload.models.insert(0, newer);
    let (signed, key) = signed(&payload);
    let catalog = TrustedCatalog::verify(&signed, &key).unwrap();
    catalog.preserves(&trusted(b"old")).unwrap();
    let upgraded = ModelManager::open(
        root.clone(),
        catalog.clone(),
        registry.clone(),
        Arc::new(|_| {}),
    )
    .unwrap();
    upgraded.initialize().await.unwrap();
    assert_eq!(
        upgraded
            .snapshot()
            .models
            .iter()
            .find(|m| m.model_id == ID)
            .unwrap()
            .state,
        InstallState::Installed
    );
    let old_lease = upgraded.lease(ID).await.unwrap();
    assert_eq!(old_lease.bundle.directory, old_path);
    // A same-size incorrect import must leave the leased old version intact.
    let corrupt = import_paths(&temp.path().join("bad-source"), b"bad");
    upgraded
        .start_import(
            &next_id,
            upgraded.prepare_import(&next_id, corrupt).unwrap(),
        )
        .unwrap();
    assert_eq!(
        settled(&upgraded).await.models[0].error,
        Some(E::HashMismatch)
    );
    assert_eq!(std::fs::read(old_path.join("model.bin")).unwrap(), b"old");
    assert!(upgraded.lease(ID).await.is_ok());

    let sources = import_paths(&temp.path().join("new-source"), b"new");
    let cancelled = upgraded
        .start_import(
            &next_id,
            upgraded.prepare_import(&next_id, sources.clone()).unwrap(),
        )
        .unwrap();
    upgraded.cancel_download(cancelled).unwrap();
    assert_eq!(
        settled(&upgraded).await.models[0].state,
        InstallState::Absent
    );
    assert!(upgraded.lease(ID).await.is_ok());
    assert_eq!(std::fs::read(old_path.join("model.bin")).unwrap(), b"old");
    upgraded
        .start_import(
            &next_id,
            upgraded.prepare_import(&next_id, sources).unwrap(),
        )
        .unwrap();
    assert_eq!(
        settled(&upgraded).await.models[0].state,
        InstallState::Installed
    );
    let new_lease = upgraded.lease(&next_id).await.unwrap();
    assert_ne!(new_lease.bundle.directory, old_path);
    assert_eq!(
        std::fs::read(new_lease.bundle.directory.join("model.bin")).unwrap(),
        b"new"
    );
    assert_eq!(upgraded.delete(ID).await, Err(E::InUse));
    assert_eq!(upgraded.delete(&next_id).await, Err(E::InUse));
    drop(new_lease);
    drop(old_lease);
    drop(upgraded);

    // Both identities survive restart; removing one never removes its sibling.
    let reopened = ModelManager::open(root, catalog, registry.clone(), Arc::new(|_| {})).unwrap();
    reopened.initialize().await.unwrap();
    assert!(reopened
        .snapshot()
        .models
        .iter()
        .all(|m| m.state == InstallState::Installed));
    reopened.delete(&next_id).await.unwrap();
    let old = reopened.lease(ID).await.unwrap();
    assert_eq!(old.bundle.directory, old_path);
    assert!(!registry.ids.lock().unwrap().contains(&next_id));
    assert!(registry.ids.lock().unwrap().iter().any(|id| id == ID));
    drop(old);
    reopened.delete(ID).await.unwrap();
    assert!(!old_path.exists());
}
#[tokio::test]
async fn whole_bundle_commit_registration_reopen_lease_and_delete() {
    let temp = tempfile::tempdir().unwrap();
    let bytes = vec![43; 200_000];
    let server = Server::ok(&bytes);
    let registry = Arc::new(RegistryProbe::default());
    let root = temp.path().join("models");
    let m = manager(root.clone(), &bytes, registry.clone(), &server);
    m.initialize().await.unwrap();
    assert_eq!(m.snapshot().models[0].state, InstallState::Absent);
    let job = m.start_download(ID).unwrap();
    assert!(matches!(m.start_download(ID), Err(E::Busy)));
    assert!(matches!(m.delete(ID).await, Err(E::Busy)));
    m.cancel_download(uuid::Uuid::new_v4()).unwrap(); // unrelated stale cancellation must not cancel this job
    let snapshot = settled(&m).await;
    assert_eq!(snapshot.models[0].state, InstallState::Installed);
    assert_eq!(snapshot.models[0].received_bytes, 400_000);
    assert_eq!(server.requests.load(Ordering::SeqCst), 2);
    assert_eq!(registry.ids.lock().unwrap().len(), 1);
    m.cancel_download(job).unwrap();
    let lease = m.lease(ID).await.unwrap();
    assert_eq!(
        std::fs::read(lease.bundle.directory.join("model.bin")).unwrap(),
        bytes
    );
    assert!(matches!(m.delete(ID).await, Err(E::InUse)));
    drop(lease);
    drop(m);
    let m = manager(root, &bytes, registry.clone(), &server);
    m.initialize().await.unwrap();
    assert_eq!(m.snapshot().models[0].state, InstallState::Installed);
    m.delete(ID).await.unwrap();
    assert!(registry.ids.lock().unwrap().is_empty());
    assert_eq!(m.snapshot().models[0].state, InstallState::Absent);
    assert_eq!(server.requests.load(Ordering::SeqCst), 2); // restart hashes local bytes without fetching
}
#[tokio::test]
async fn failure_states_never_register_incomplete_or_wrong_bytes() {
    for (response, error, requests) in [
        (b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(), E::HttpStatus, 1),
        (b"HTTP/1.1 206 Partial Content\r\nContent-Length: 7\r\nConnection: close\r\n\r\nweights".to_vec(), E::HttpStatus, 1),
        (b"HTTP/1.1 500 Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(), E::Server, 3),
        (b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\ncorrupt".to_vec(), E::HashMismatch, 1),
        (b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nweights!".to_vec(), E::SizeMismatch, 1),
        (b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n8\r\nweights!\r\n0\r\n\r\n".to_vec(), E::SizeMismatch, 1),
        (b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nshort".to_vec(), E::SizeMismatch, 1),
    ] {
        let temp = tempfile::tempdir().unwrap(); let server = Server::new(response, Duration::ZERO); let registry = Arc::new(RegistryProbe::default());
        let m = manager(temp.path().join("models"), b"weights", registry.clone(), &server); m.initialize().await.unwrap(); m.start_download(ID).unwrap();
        assert_eq!(settled(&m).await.models[0].error, Some(error)); assert_eq!(server.requests.load(Ordering::SeqCst), requests, "request count for {error:?}");
        assert!(registry.ids.lock().unwrap().is_empty()); assert_eq!(std::fs::read_dir(m.0.root.join("staging")).unwrap().count(), 0);
    }
}
#[tokio::test]
async fn cancellation_interrupts_waiting_request_and_cleans_staging() {
    let temp = tempfile::tempdir().unwrap();
    let server = Server::new(Vec::new(), Duration::from_secs(3));
    let registry = Arc::new(RegistryProbe::default());
    let m = manager(
        temp.path().join("models"),
        b"weights",
        registry.clone(),
        &server,
    );
    m.initialize().await.unwrap();
    let job = m.start_download(ID).unwrap();
    while server.requests.load(Ordering::SeqCst) == 0 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let start = Instant::now();
    m.cancel_download(job).unwrap();
    assert_eq!(settled(&m).await.models[0].state, InstallState::Absent);
    assert!(start.elapsed() < Duration::from_millis(500));
    assert_eq!(
        std::fs::read_dir(m.0.root.join("staging")).unwrap().count(),
        0
    );
    assert!(registry.ids.lock().unwrap().is_empty());
}
#[tokio::test]
async fn network_timeout_is_bounded_and_retried_only_twice() {
    let temp = tempfile::tempdir().unwrap();
    let server = Server::new(Vec::new(), Duration::from_millis(175));
    let registry = Arc::new(RegistryProbe::default());
    let m = manager_with_timeout(
        temp.path().join("models"),
        b"weights",
        registry,
        &server,
        Duration::from_millis(150),
    );
    m.initialize().await.unwrap();
    m.start_download(ID).unwrap();
    assert_eq!(settled(&m).await.models[0].error, Some(E::Network));
    assert_eq!(server.requests.load(Ordering::SeqCst), 3);
}
#[tokio::test]
async fn registration_failure_preserves_verified_orphan_and_recovers_without_network() {
    let temp = tempfile::tempdir().unwrap();
    let server = Server::ok(b"weights");
    let registry = Arc::new(RegistryProbe::default());
    registry.fail.store(true, Ordering::SeqCst);
    let root = temp.path().join("models");
    let m = manager(root.clone(), b"weights", registry.clone(), &server);
    m.initialize().await.unwrap();
    m.start_download(ID).unwrap();
    assert_eq!(settled(&m).await.models[0].error, Some(E::Registration));
    let path = m.path(m.0.catalog.model(ID).unwrap());
    assert!(path.join("vad.bin").is_file());
    drop(m);
    registry.fail.store(false, Ordering::SeqCst);
    let m = manager(root.clone(), b"weights", registry.clone(), &server);
    m.initialize().await.unwrap();
    assert_eq!(m.snapshot().models[0].state, InstallState::Installed);
    assert_eq!(server.requests.load(Ordering::SeqCst), 2);
    std::fs::write(path.join("model.bin"), b"corrupt").unwrap();
    assert!(matches!(m.lease(ID).await, Err(E::HashMismatch)));
    assert!(registry.ids.lock().unwrap().is_empty());
    drop(m);
    let staging = root
        .join("staging")
        .join(format!("job-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&staging).unwrap();
    std::fs::write(staging.join("partial.part"), b"incomplete").unwrap();
    let m = manager(root, b"weights", registry, &server);
    m.initialize().await.unwrap();
    assert!(!staging.exists());
    assert_eq!(m.snapshot().models[0].state, InstallState::Invalid);
    m.start_download(ID).unwrap();
    assert_eq!(settled(&m).await.models[0].state, InstallState::Installed);
}
#[tokio::test]
async fn commit_boundary_rejects_cancel_and_directory_lock_rejects_second_owner() {
    let temp = tempfile::tempdir().unwrap();
    let server = Server::ok(b"weights");
    let registry = Arc::new(RegistryProbe::default());
    let root = temp.path().join("models");
    let m = manager(root.clone(), b"weights", registry.clone(), &server);
    m.initialize().await.unwrap();
    assert!(matches!(
        ModelManager::open(root, trusted(b"weights"), registry, Arc::new(|_| {})),
        Err(E::Busy)
    ));
    let (id, token) = m
        .reserve(m.0.catalog.model(ID).unwrap(), true, JobKind::Download)
        .unwrap();
    m.begin_commit(id, &token).unwrap();
    assert_eq!(m.cancel_download(id).unwrap_err(), E::Committing);
    assert!(!token.is_cancelled());
    m.finish(id);
    let (id, token) = m
        .reserve(m.0.catalog.model(ID).unwrap(), true, JobKind::Import)
        .unwrap();
    m.cancel_download(id).unwrap();
    assert_eq!(m.begin_commit(id, &token).unwrap_err(), E::Cancelled);
    m.finish(id);
    assert_eq!(
        fs::check_space(u64::MAX, u64::MAX).unwrap_err(),
        E::InsufficientSpace
    );
    assert!(fs::check_space(64 * 1024 * 1024 + 7, 7).is_ok());
}
#[cfg(unix)]
#[tokio::test]
async fn symlinks_are_never_loaded_or_followed_for_deletion() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let server = Server::ok(b"weights");
    let registry = Arc::new(RegistryProbe::default());
    let external = temp.path().join("external");
    std::fs::create_dir(&external).unwrap();
    std::fs::write(external.join("keep"), b"private").unwrap();
    let root = temp.path().join("models");
    symlink(&external, &root).unwrap();
    assert!(matches!(
        ModelManager::open(
            root.clone(),
            trusted(b"weights"),
            registry.clone(),
            Arc::new(|_| {})
        ),
        Err(E::Storage)
    ));
    std::fs::remove_file(&root).unwrap();
    let m = manager(root, b"weights", registry, &server);
    m.initialize().await.unwrap();
    let parent = m.0.root.join("bundles").join(ID);
    symlink(&external, &parent).unwrap();
    m.start_download(ID).unwrap();
    assert_eq!(settled(&m).await.models[0].error, Some(E::Storage));
    assert_eq!(std::fs::read(external.join("keep")).unwrap(), b"private");
}

#[tokio::test]
async fn concurrent_snapshots_never_reuse_a_revision_for_changed_data() {
    let temp = tempfile::tempdir().unwrap();
    let server = Server::ok(b"weights");
    let m = manager(
        temp.path().join("models"),
        b"weights",
        Arc::new(RegistryProbe::default()),
        &server,
    );
    m.initialize().await.unwrap();
    assert_eq!(m.initialize().await.unwrap_err(), E::Busy);
    let start = Arc::new(std::sync::Barrier::new(3));
    let done = Arc::new(AtomicBool::new(false));
    let mut readers = vec![];
    for _ in 0..2 {
        let owner = m.clone();
        let start = start.clone();
        let done = done.clone();
        readers.push(thread::spawn(move || {
            start.wait();
            let mut previous = None;
            while !done.load(Ordering::SeqCst) {
                let s = owner.snapshot();
                let values = (
                    s.models[0].state,
                    s.models[0].received_bytes,
                    s.busy,
                    s.cancellable,
                );
                if let Some((revision, old)) = previous {
                    assert!(s.revision >= revision);
                    if s.revision == revision {
                        assert_eq!(
                            old, values,
                            "same revision cannot identify two different snapshots"
                        );
                    }
                }
                previous = Some((s.revision, values));
                thread::yield_now();
            }
        }));
    }
    start.wait();
    for received in 0..5000 {
        m.row(ID, InstallState::Downloading, received, 1, None);
        thread::yield_now();
    }
    done.store(true, Ordering::SeqCst);
    for reader in readers {
        reader.join().unwrap();
    }
}

fn import_paths(dir: &Path, bytes: &[u8]) -> Vec<PathBuf> {
    std::fs::create_dir_all(dir).unwrap();
    ["model.bin", "vad.bin"]
        .into_iter()
        .map(|name| {
            let path = dir.join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        })
        .collect()
}
#[tokio::test]
async fn offline_import_pins_selected_files_keeps_originals_and_reopens_without_network() {
    let temp = tempfile::tempdir().unwrap();
    let bytes = vec![47; 200_000];
    let server = Server::ok(b"must not fetch");
    let registry = Arc::new(RegistryProbe::default());
    let root = temp.path().join("models");
    let m = manager(root.clone(), &bytes, registry.clone(), &server);
    m.initialize().await.unwrap();
    let files = import_paths(&temp.path().join("source"), &bytes);
    let source = m.prepare_import(ID, files.clone()).unwrap();
    let moved = files[0].with_file_name("original-model.bin");
    std::fs::rename(&files[0], &moved).unwrap();
    std::fs::write(&files[0], vec![0; bytes.len()]).unwrap();
    let id = m.start_import(ID, source).unwrap();
    assert!(matches!(m.snapshot().job_kind, Some(JobKind::Import)));
    assert!(matches!(m.start_download(ID), Err(E::Busy)));
    m.cancel_download(Uuid::new_v4()).unwrap();
    assert_eq!(settled(&m).await.models[0].state, InstallState::Installed);
    assert_eq!(std::fs::read(&moved).unwrap(), bytes);
    assert_eq!(std::fs::read(&files[1]).unwrap(), bytes);
    assert_eq!(std::fs::read(&files[0]).unwrap(), vec![0; bytes.len()]);
    assert_eq!(
        std::fs::read(m.path(m.0.catalog.model(ID).unwrap()).join("model.bin")).unwrap(),
        bytes
    );
    let prepared = m
        .prepare_import(
            ID,
            vec![moved.with_file_name("model.bin"), files[1].clone()],
        )
        .unwrap();
    assert!(matches!(m.start_import(ID, prepared), Err(E::InUse)));
    m.cancel_download(id).unwrap();
    assert_eq!(server.requests.load(Ordering::SeqCst), 0);
    drop(m);
    let m = manager(root, &bytes, registry, &server);
    m.initialize().await.unwrap();
    let lease = m.lease(ID).await.unwrap();
    assert_eq!(
        std::fs::read(lease.bundle.directory.join("vad.bin")).unwrap(),
        bytes
    );
    assert_eq!(server.requests.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn local_selection_rejects_incomplete_duplicate_wrong_name_size_and_nonregular_files() {
    let temp = tempfile::tempdir().unwrap();
    let server = Server::ok(b"weights");
    let m = manager(
        temp.path().join("models"),
        b"weights",
        Arc::default(),
        &server,
    );
    m.initialize().await.unwrap();
    let files = import_paths(&temp.path().join("source"), b"weights");
    for selection in [
        vec![],
        vec![files[0].clone()],
        vec![files[0].clone(), files[0].clone()],
        vec![PathBuf::from("model.bin"), files[1].clone()],
    ] {
        assert!(matches!(
            m.prepare_import(ID, selection),
            Err(E::ImportSelection)
        ));
    }
    let other = files[0].with_file_name("renamed.bin");
    std::fs::write(&other, b"weights").unwrap();
    assert!(matches!(
        m.prepare_import(ID, vec![other, files[1].clone()]),
        Err(E::ImportSelection)
    ));
    std::fs::write(&files[1], b"truncated").unwrap();
    assert!(matches!(
        m.prepare_import(ID, files.clone()),
        Err(E::SizeMismatch)
    ));
    std::fs::remove_file(&files[1]).unwrap();
    std::fs::create_dir(&files[1]).unwrap();
    assert!(matches!(
        m.prepare_import(ID, files.clone()),
        Err(E::Storage)
    ));
    std::fs::remove_dir(&files[1]).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&files[0], &files[1]).unwrap();
        assert!(matches!(
            m.prepare_import(ID, files.clone()),
            Err(E::Storage)
        ));
        std::fs::remove_file(&files[1]).unwrap();
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(files[1].as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        assert!(matches!(m.prepare_import(ID, files), Err(E::Storage)));
    }
    assert_eq!(m.snapshot().models[0].state, InstallState::Absent);
    assert!(!m.snapshot().busy);
    assert_eq!(server.requests.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn import_rejects_changes_after_selection_and_never_commits_partial_bytes() {
    for (replacement, expected) in [
        (b"corrupt".as_slice(), E::HashMismatch),
        (b"short".as_slice(), E::SizeMismatch),
        (b"weights too long".as_slice(), E::SizeMismatch),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let server = Server::ok(b"weights");
        let registry = Arc::new(RegistryProbe::default());
        let m = manager(
            temp.path().join("models"),
            b"weights",
            registry.clone(),
            &server,
        );
        m.initialize().await.unwrap();
        let files = import_paths(&temp.path().join("source"), b"weights");
        let source = m.prepare_import(ID, files.clone()).unwrap();
        std::fs::write(&files[1], replacement).unwrap();
        m.start_import(ID, source).unwrap();
        let s = settled(&m).await;
        assert_eq!(s.models[0].error, Some(expected));
        assert!(registry.ids.lock().unwrap().is_empty());
        assert!(!m.path(m.0.catalog.model(ID).unwrap()).exists());
        assert_eq!(
            std::fs::read_dir(m.0.root.join("staging")).unwrap().count(),
            0
        );
        assert_eq!(std::fs::read(&files[1]).unwrap(), replacement);
        assert_eq!(server.requests.load(Ordering::SeqCst), 0);
    }
}
#[tokio::test]
async fn cancelled_import_cleans_staging_and_allows_explicit_retry() {
    let temp = tempfile::tempdir().unwrap();
    let bytes = vec![29; 2_000_000];
    let server = Server::ok(&bytes);
    let registry = Arc::new(RegistryProbe::default());
    let mut m = manager(
        temp.path().join("models"),
        &bytes,
        registry.clone(),
        &server,
    );
    let observed = Arc::new(AtomicBool::new(false));
    let watcher: Arc<Mutex<Option<std::sync::Weak<Inner>>>> = Arc::default();
    let callback_watcher = watcher.clone();
    let cancelled = observed.clone();
    Arc::get_mut(&mut m.0).unwrap().notify = Arc::new(move |_| {
        if cancelled.load(Ordering::SeqCst) {
            return;
        }
        let Some(inner) = callback_watcher
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|w| w.upgrade())
        else {
            return;
        };
        let owner = ModelManager(inner);
        let state = owner.snapshot();
        if state.models[0].state == InstallState::Importing
            && state.models[0].received_bytes > 0
            && !cancelled.swap(true, Ordering::SeqCst)
        {
            owner.cancel_download(state.active_job_id.unwrap()).unwrap();
        }
    });
    *watcher.lock().unwrap() = Some(Arc::downgrade(&m.0));
    m.initialize().await.unwrap();
    let files = import_paths(&temp.path().join("source"), &bytes);
    let source = m.prepare_import(ID, files.clone()).unwrap();
    m.start_import(ID, source).unwrap();
    assert_eq!(settled(&m).await.models[0].state, InstallState::Absent);
    assert!(observed.load(Ordering::SeqCst));
    assert!(registry.ids.lock().unwrap().is_empty());
    assert_eq!(
        std::fs::read_dir(m.0.root.join("staging")).unwrap().count(),
        0
    );
    m.start_import(ID, m.prepare_import(ID, files.clone()).unwrap())
        .unwrap();
    assert_eq!(settled(&m).await.models[0].state, InstallState::Installed);
    for file in files {
        assert_eq!(std::fs::read(file).unwrap(), bytes);
    }
    assert_eq!(server.requests.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn import_registration_failure_recovers_verified_bundle_on_restart() {
    let temp = tempfile::tempdir().unwrap();
    let server = Server::ok(b"weights");
    let registry = Arc::new(RegistryProbe::default());
    let root = temp.path().join("models");
    let m = manager(root.clone(), b"weights", registry.clone(), &server);
    m.initialize().await.unwrap();
    registry.fail.store(true, Ordering::SeqCst);
    let files = import_paths(&temp.path().join("source"), b"weights");
    m.start_import(ID, m.prepare_import(ID, files).unwrap())
        .unwrap();
    assert_eq!(settled(&m).await.models[0].error, Some(E::Registration));
    assert!(registry.ids.lock().unwrap().is_empty());
    drop(m);
    registry.fail.store(false, Ordering::SeqCst);
    let m = manager(root, b"weights", registry.clone(), &server);
    m.initialize().await.unwrap();
    assert_eq!(m.snapshot().models[0].state, InstallState::Installed);
    assert_eq!(registry.ids.lock().unwrap().len(), 1);
    assert_eq!(server.requests.load(Ordering::SeqCst), 0);
}
fn full_hashes(m: &ModelManager) -> usize {
    m.0.full_hashes.load(Ordering::SeqCst)
}
async fn imported(temp: &Path, bytes: &[u8]) -> (PathBuf, Arc<RegistryProbe>) {
    let root = temp.join("models");
    let registry = Arc::new(RegistryProbe::default());
    let server = Server::ok(b"unused");
    let m = manager(root.clone(), bytes, registry.clone(), &server);
    m.initialize().await.unwrap();
    let files = import_paths(&temp.join("source"), bytes);
    m.start_import(ID, m.prepare_import(ID, files).unwrap())
        .unwrap();
    assert_eq!(settled(&m).await.models[0].state, InstallState::Installed);
    (root, registry)
}
#[tokio::test]
async fn verified_bundles_are_not_rehashed_until_their_files_change() {
    let temp = tempfile::tempdir().unwrap();
    let bytes = vec![11; 300_000];
    let server = Server::ok(b"unused");
    let registry = Arc::new(RegistryProbe::default());
    let m = manager(temp.path().join("models"), &bytes, registry, &server);
    m.initialize().await.unwrap();
    let files = import_paths(&temp.path().join("source"), &bytes);
    m.start_import(ID, m.prepare_import(ID, files).unwrap())
        .unwrap();
    settled(&m).await;
    // The import's own full verification covers the first lease and later reloads.
    let directory = m.lease(ID).await.unwrap().bundle.directory.clone();
    m.lease(ID).await.unwrap();
    assert_eq!(full_hashes(&m), 0);
    // Finder metadata is tolerated and does not invalidate the verified files.
    std::fs::write(directory.join(".DS_Store"), b"finder").unwrap();
    std::fs::write(directory.join("._model.bin"), b"appledouble").unwrap();
    m.lease(ID).await.unwrap();
    assert_eq!(full_hashes(&m), 0);
    // Any write changes the file's change time: identical bytes are hashed again.
    std::fs::write(directory.join("vad.bin"), &bytes).unwrap();
    m.lease(ID).await.unwrap();
    assert_eq!(full_hashes(&m), 1);
    m.lease(ID).await.unwrap();
    assert_eq!(full_hashes(&m), 1);
    std::fs::write(directory.join("model.bin"), vec![12; bytes.len()]).unwrap();
    assert!(matches!(m.lease(ID).await, Err(E::HashMismatch)));
    assert_eq!(full_hashes(&m), 1);
}
#[tokio::test]
async fn unexpected_bundle_entries_are_rejected_but_finder_metadata_survives_restart() {
    let temp = tempfile::tempdir().unwrap();
    let (root, registry) = imported(temp.path(), b"weights").await;
    let server = Server::ok(b"unused");
    let m = manager(root.clone(), b"weights", registry.clone(), &server);
    let directory = m.path(m.0.catalog.model(ID).unwrap());
    std::fs::write(directory.join(".DS_Store"), b"finder").unwrap();
    m.initialize().await.unwrap();
    assert_eq!(m.snapshot().models[0].state, InstallState::Installed);
    assert_eq!(full_hashes(&m), 1);
    std::fs::create_dir(directory.join("._folder")).unwrap();
    assert!(matches!(m.lease(ID).await, Err(E::SizeMismatch)));
    std::fs::remove_dir(directory.join("._folder")).unwrap();
    drop(m);
    let m = manager(root, b"weights", registry, &server);
    std::fs::write(directory.join("notes.txt"), b"extra").unwrap();
    m.initialize().await.unwrap();
    assert_eq!(m.snapshot().models[0].error, Some(E::SizeMismatch));
}
#[tokio::test]
async fn a_lease_claims_its_own_bundle_while_startup_hashing_continues() {
    let temp = tempfile::tempdir().unwrap();
    let (root, registry) = imported(temp.path(), b"weights").await;
    let server = Server::ok(b"unused");
    let m = manager(root, b"weights", registry.clone(), &server);
    // State after staging cleanup: other mutations are allowed, this bundle is not.
    m.update(|state| {
        state.initialization_started = true;
        state.initializing = false;
        state.pending = vec![ID.into()];
    });
    assert!(!m.snapshot().busy);
    assert_eq!(m.snapshot().models[0].state, InstallState::Checking);
    assert_eq!(m.delete(ID).await, Err(E::Busy));
    assert!(matches!(m.start_download(ID), Err(E::Busy)));
    let lease = m.lease(ID).await.unwrap();
    assert!(lease.bundle.directory.join("model.bin").is_file());
    assert_eq!(m.snapshot().models[0].state, InstallState::Installed);
    assert!(m.0.state.lock().unwrap().pending.is_empty());
    assert_eq!(full_hashes(&m), 1);
    // The claimed startup check also reconciled the registration.
    assert_eq!(registry.ids.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn a_lease_waiting_on_a_failed_startup_check_reports_its_error() {
    let temp = tempfile::tempdir().unwrap();
    let (root, registry) = imported(temp.path(), b"weights").await;
    let server = Server::ok(b"unused");
    let m = manager(root, b"weights", registry, &server);
    std::fs::write(
        m.path(m.0.catalog.model(ID).unwrap()).join("model.bin"),
        b"weightz",
    )
    .unwrap();
    m.update(|state| {
        state.initialization_started = true;
        state.initializing = false;
        state.pending = vec![ID.into()];
    });
    assert!(matches!(m.lease(ID).await, Err(E::HashMismatch)));
    let row = &m.snapshot().models[0];
    assert_eq!(row.state, InstallState::Invalid);
    assert_eq!(row.error, Some(E::HashMismatch));
}
#[tokio::test]
async fn retained_download_resume_data_is_dropped_by_import_and_delete() {
    let temp = tempfile::tempdir().unwrap();
    let server = Server::ok(b"unused");
    let m = manager(
        temp.path().join("models"),
        b"weights",
        Arc::default(),
        &server,
    );
    m.initialize().await.unwrap();
    let resume = m.resume_staging(m.0.catalog.model(ID).unwrap());
    let retain = || {
        std::fs::create_dir_all(&resume).unwrap();
        std::fs::write(resume.join("model.bin.part"), b"wei").unwrap();
    };
    retain();
    let files = import_paths(&temp.path().join("source"), b"weights");
    m.start_import(ID, m.prepare_import(ID, files).unwrap())
        .unwrap();
    assert_eq!(settled(&m).await.models[0].state, InstallState::Installed);
    assert!(!resume.exists());
    retain();
    m.delete(ID).await.unwrap();
    assert!(!resume.exists());
    assert_eq!(server.requests.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn unsupported_installed_models_remain_removable() {
    let temp = tempfile::tempdir().unwrap();
    let (root, registry) = imported(temp.path(), b"weights").await;
    let server = Server::ok(b"unused");
    let m = manager(root, b"weights", registry.clone(), &server);
    m.initialize().await.unwrap();
    m.update(|state| state.rows.get_mut(ID).unwrap().supported = false);
    assert!(matches!(m.lease(ID).await, Err(E::UnsupportedModel)));
    m.delete(ID).await.unwrap();
    assert_eq!(m.snapshot().models[0].state, InstallState::Absent);
    assert!(registry.ids.lock().unwrap().is_empty());
    assert!(matches!(m.start_download(ID), Err(E::UnsupportedModel)));
}
#[tokio::test]
async fn unsafe_staging_reports_storage_instead_of_staying_busy() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("models");
    let server = Server::ok(b"weights");
    let m = manager(root.clone(), b"weights", Arc::default(), &server);
    let job = root.join("staging").join(format!("job-{}", Uuid::new_v4()));
    std::fs::create_dir_all(job.join("nested")).unwrap();
    assert_eq!(m.initialize().await.unwrap_err(), E::Storage);
    let snapshot = m.snapshot();
    assert!(!snapshot.busy);
    assert!(snapshot
        .models
        .iter()
        .all(|row| row.state == InstallState::Invalid && row.error == Some(E::Storage)));
    assert!(matches!(m.start_download(ID), Err(E::Storage)));
    assert!(matches!(m.lease(ID).await, Err(E::Storage)));
    assert!(job.join("nested").is_dir());
    assert_eq!(server.requests.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn interrupted_downloads_resume_by_range_and_keep_completed_files_for_retry() {
    let temp = tempfile::tempdir().unwrap();
    let bytes: Vec<u8> = (0..400_000_u32).map(|i| (i % 251) as u8).collect();
    let served = bytes.clone();
    let offsets = Arc::new(Mutex::new(Vec::new()));
    let seen = offsets.clone();
    let vad_ready = Arc::new(AtomicBool::new(false));
    let ready = vad_ready.clone();
    let truncated = Arc::new(AtomicBool::new(false));
    let server = Server::with_handler(
        Arc::new(move |head: &str| {
            let offset = head
                .split("range: bytes=")
                .nth(1)
                .and_then(|r| r.split('-').next())
                .map_or(0, |n| n.parse::<usize>().unwrap());
            seen.lock()
                .unwrap()
                .push((head.contains("/vad.bin"), offset));
            if head.contains("/vad.bin") && !ready.load(Ordering::SeqCst) {
                return b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_vec();
            }
            let mut response = if offset == 0 {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    served.len()
                )
            } else {
                format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {offset}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    served.len() - 1,
                    served.len(),
                    served.len() - offset
                )
            }
            .into_bytes();
            // The first model request ends early, after sending part of the body.
            let end = if offset == 0 && !truncated.swap(true, Ordering::SeqCst) {
                150_000
            } else {
                served.len()
            };
            response.extend_from_slice(&served[offset..end]);
            response
        }),
        Duration::ZERO,
    );
    let registry = Arc::new(RegistryProbe::default());
    let m = manager(
        temp.path().join("models"),
        &bytes,
        registry.clone(),
        &server,
    );
    m.initialize().await.unwrap();
    m.start_download(ID).unwrap();
    let failed = settled(&m).await;
    assert_eq!(failed.models[0].state, InstallState::DownloadFailed);
    assert_eq!(failed.models[0].error, Some(E::Server));
    {
        let requests = offsets.lock().unwrap();
        let model: Vec<_> = requests.iter().filter(|(vad, _)| !vad).collect();
        assert_eq!(model.len(), 2);
        assert!(model[1].1 > 0, "the retry must resume from flushed bytes");
        assert_eq!(requests.iter().filter(|(vad, _)| *vad).count(), 3);
    }
    let staging = std::fs::read_dir(m.0.root.join("staging"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(staging.len(), 1);
    assert!(staging[0].join("model.bin").is_file());
    vad_ready.store(true, Ordering::SeqCst);
    m.start_download(ID).unwrap();
    assert_eq!(settled(&m).await.models[0].state, InstallState::Installed);
    let requests = offsets.lock().unwrap().clone();
    assert_eq!(requests.iter().filter(|(vad, _)| !vad).count(), 2);
    assert_eq!(requests.len(), 6);
    assert_eq!(registry.ids.lock().unwrap().len(), 1);
    assert_eq!(
        std::fs::read_dir(m.0.root.join("staging")).unwrap().count(),
        0
    );
    let lease = m.lease(ID).await.unwrap();
    assert_eq!(
        std::fs::read(lease.bundle.directory.join("model.bin")).unwrap(),
        bytes
    );
}
#[test]
fn only_rate_limits_timeouts_and_server_faults_are_temporary() {
    use reqwest::StatusCode as S;
    for status in [S::TOO_MANY_REQUESTS, S::REQUEST_TIMEOUT, S::BAD_GATEWAY] {
        assert_eq!(status_error(status), E::Server);
    }
    for status in [
        S::NOT_FOUND,
        S::FORBIDDEN,
        S::UNAUTHORIZED,
        S::PARTIAL_CONTENT,
    ] {
        assert_eq!(status_error(status), E::HttpStatus);
    }
}
