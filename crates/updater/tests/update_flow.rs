use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use std::path::Path;
use swoop_updater::{Artifact, Candidate, Channel, Release, SignedFeed, UpdateConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

/// 测试夹具的“新版本”：始终比当前 crate 版本高一个 patch，保证“只允许升级”校验在发版后依然成立。
fn feed_version() -> String {
    let mut parts: Vec<u64> = env!("CARGO_PKG_VERSION").split('.').map(|part| part.parse().unwrap()).collect();
    *parts.last_mut().unwrap() += 1;
    parts.iter().map(u64::to_string).collect::<Vec<_>>().join(".")
}

fn signed(url: &str, bytes: &[u8]) -> (SignedFeed, String) {
    let key = SigningKey::from_bytes(&[19; 32]); // Public deterministic test fixture only.
    let release = Release {
        version: feed_version(),
        channel: Channel::Stable,
        notes: "Test update".into(),
        platforms: BTreeMap::from([(
            swoop_updater::platform(),
            Artifact { url: url.into(), size: bytes.len() as u64, sha256: swoop_updater::hex(&Sha256::digest(bytes)) },
        )]),
    };
    let payload = serde_json::to_string(&release).unwrap();
    let signature = swoop_updater::hex(&key.sign(payload.as_bytes()).to_bytes());
    (SignedFeed { payload, signature }, swoop_updater::hex(&key.verifying_key().to_bytes()))
}
fn candidate(bytes: &[u8]) -> Candidate {
    let (feed, key) = signed("https://updates.test/download.zip", bytes);
    swoop_updater::verify(&feed, &key, env!("CARGO_PKG_VERSION"), &Channel::Stable, &swoop_updater::platform())
        .unwrap()
        .unwrap()
}

#[test]
fn signature_version_channel_platform_and_configuration_are_enforced() {
    let (feed, key) = signed("https://updates.test/p.zip", b"archive");
    let current = env!("CARGO_PKG_VERSION");
    let target = swoop_updater::platform();
    assert!(swoop_updater::verify(&feed, &key, current, &Channel::Stable, &target).unwrap().is_some());
    assert!(swoop_updater::verify(&feed, &key, &feed_version(), &Channel::Stable, &target).unwrap().is_none());
    assert!(swoop_updater::verify(&feed, &key, current, &Channel::Preview, &target).unwrap().is_none());
    assert!(swoop_updater::verify(&feed, &key, current, &Channel::Stable, "unknown-target").is_err());
    let mut changed = feed.clone();
    changed.payload.push(' ');
    assert!(swoop_updater::verify(&changed, &key, current, &Channel::Stable, &target).is_err());
    assert!(swoop_updater::verify(&feed, &"00".repeat(32), current, &Channel::Stable, &target).is_err());
    for url in
        ["file:///tmp/feed", "http://public.test/feed", "https://user:password@a.test/feed", "https://a.test/#fragment"]
    {
        assert!(
            UpdateConfig { feed_url: url.into(), public_key: key.clone(), ..Default::default() }.validate().is_err(),
            "{url}"
        );
    }
    assert!(UpdateConfig::default().validate().is_ok());
}

#[test]
fn built_in_official_defaults_need_no_user_imported_key() {
    let config = UpdateConfig::default();
    assert_eq!(config.feed_url, swoop_updater::DEFAULT_FEED_URL);
    assert_eq!(config.public_key, swoop_updater::OFFICIAL_PUBLIC_KEY);
    assert!(config.feed_url.starts_with("https://github.com/i-mufeng/muffin-swoop/"));
    assert_ne!(swoop_updater::OFFICIAL_PUBLIC_KEY, swoop_updater::LEGACY_PUBLIC_KEY);
    // 内置公钥必须是合法 Ed25519 公钥（validate 内部完成解码校验）。
    assert!(config.validate().is_ok());
    // 内置公钥与固定测试密钥不同，测试密钥签发的清单不能冒充官方清单。
    let (feed, fixture_key) = signed("https://updates.test/p.zip", b"archive");
    assert_ne!(fixture_key, swoop_updater::OFFICIAL_PUBLIC_KEY);
    assert!(
        swoop_updater::verify(
            &feed,
            swoop_updater::OFFICIAL_PUBLIC_KEY,
            env!("CARGO_PKG_VERSION"),
            &Channel::Stable,
            &swoop_updater::platform(),
        )
        .is_err()
    );
}

#[test]
fn migration_fills_blank_values_and_replaces_rotated_legacy_key() {
    for (feed_url, public_key, expected_feed) in [
        (String::new(), String::new(), swoop_updater::DEFAULT_FEED_URL),
        (String::new(), swoop_updater::OFFICIAL_PUBLIC_KEY.into(), swoop_updater::DEFAULT_FEED_URL),
        (
            "https://mirror.example.com/feed.json".into(),
            String::new(),
            "https://mirror.example.com/feed.json",
        ),
        (
            "https://mirror.example.com/feed.json".into(),
            swoop_updater::LEGACY_PUBLIC_KEY.into(),
            "https://mirror.example.com/feed.json",
        ),
        (
            "https://mirror.example.com/feed.json".into(),
            swoop_updater::LEGACY_PUBLIC_KEY.to_uppercase(),
            "https://mirror.example.com/feed.json",
        ),
    ] {
        let mut config = UpdateConfig { feed_url, public_key, ..Default::default() };
        config.migrate();
        assert_eq!(config.feed_url, expected_feed);
        assert_eq!(config.public_key, swoop_updater::OFFICIAL_PUBLIC_KEY);
    }
    // 自定义源与自定义公钥原样保留，不被内置值覆盖。
    let mut custom =
        UpdateConfig { feed_url: "https://mirror.example.com/feed.json".into(), public_key: "ab".repeat(32), ..Default::default() };
    custom.migrate();
    assert_eq!(custom.feed_url, "https://mirror.example.com/feed.json");
    assert_eq!(custom.public_key, "ab".repeat(32));
    // 迁移幂等。
    let before = custom.clone();
    custom.migrate();
    assert_eq!(custom, before);
}

struct Server {
    base: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn server(body: Vec<u8>, declared: Option<usize>) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else { break };
            let mut input = [0_u8; 8192];
            let _ = stream.read(&mut input).await;
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                declared.unwrap_or(body.len())
            );
            let _ = stream.write_all(headers.as_bytes()).await;
            let _ = stream.write_all(&body).await;
            let _ = stream.shutdown().await;
        }
    });
    Server { base, task }
}

#[tokio::test]
async fn real_http_signed_feed_and_verified_download() {
    let body = b"signed package payload";
    let files = server(body.to_vec(), None).await;
    let (feed, key) = signed(&format!("{}/package.zip", files.base), body);
    let feeds = server(serde_json::to_vec(&feed).unwrap(), None).await;
    let config = UpdateConfig { feed_url: feeds.base.clone(), public_key: key, ..Default::default() };
    let cancel = CancellationToken::new();
    let found = swoop_updater::check(&config, env!("CARGO_PKG_VERSION"), &cancel).await.unwrap().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = swoop_updater::download(&found, directory.path(), &cancel).await.unwrap();
    assert_eq!(std::fs::read(path).unwrap(), body);
}

#[tokio::test]
async fn wrong_hash_short_response_and_cancellation_never_publish_a_package() {
    for (body, declared) in [(b"BAD".to_vec(), None), (b"x".to_vec(), Some(3))] {
        let files = server(body, declared).await;
        let (feed, key) = signed(&files.base, b"ZIP");
        let found =
            swoop_updater::verify(&feed, &key, env!("CARGO_PKG_VERSION"), &Channel::Stable, &swoop_updater::platform())
                .unwrap()
                .unwrap();
        let directory = tempfile::tempdir().unwrap();
        assert!(swoop_updater::download(&found, directory.path(), &CancellationToken::new()).await.is_err());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
    let directory = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(swoop_updater::download(&candidate(b"ZIP"), directory.path(), &cancel).await.is_err());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, body) in entries {
        zip.start_file(*name, zip::write::SimpleFileOptions::default().unix_permissions(0o755)).unwrap();
        zip.write_all(body).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[cfg(target_os = "macos")]
fn installation(directory: &Path) -> std::path::PathBuf {
    let executable = directory.join("installed/Swoop.app/Contents/MacOS/swoop");
    std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
    std::fs::write(&executable, b"old executable").unwrap();
    std::fs::write(directory.join("installed/Swoop.app/Contents/Info.plist"), b"old plist").unwrap();
    std::fs::write(directory.join("installed/swoop-cli"), b"old cli").unwrap();
    std::fs::write(directory.join("installed/user-file.txt"), b"user file").unwrap();
    executable
}
#[cfg(target_os = "macos")]
fn good_archive() -> Vec<u8> {
    archive_with_executable(b"new executable")
}
#[cfg(target_os = "macos")]
fn archive_with_executable(executable: &[u8]) -> Vec<u8> {
    let build = format!(
        "name=Swoop\napplication_id=com.muffin.swoop\nversion={}\ntarget={}-apple-darwin\nbinary_sha256={}\ncli_sha256={}\n",
        feed_version(),
        std::env::consts::ARCH,
        swoop_updater::hex(&Sha256::digest(executable)),
        swoop_updater::hex(&Sha256::digest(b"new cli"))
    );
    archive(&[
        ("Swoop-test/Swoop.app/Contents/MacOS/swoop", executable),
        ("Swoop-test/Swoop.app/Contents/Info.plist", b"new plist"),
        ("Swoop-test/swoop-cli", b"new cli"),
        ("Swoop-test/build-info.txt", build.as_bytes()),
    ])
}
#[cfg(target_os = "macos")]
fn plan(directory: &Path, bytes: &[u8]) -> swoop_updater::InstallPlan {
    let executable = installation(directory);
    let path = directory.join("update.zip");
    std::fs::write(&path, bytes).unwrap();
    let data = directory.join("data");
    std::fs::create_dir(&data).unwrap();
    let (path, _, _) = swoop_updater::prepare(&candidate(bytes), &path, &executable, &data).unwrap();
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
#[cfg(target_os = "macos")]
fn installation_replaces_only_app_and_cli_and_keeps_backups() {
    let directory = tempfile::tempdir().unwrap();
    let plan = plan(directory.path(), &good_archive());
    let backups = swoop_updater::apply(&plan).unwrap();
    assert_eq!(std::fs::read(&plan.executable).unwrap(), b"new executable");
    assert_eq!(std::fs::read(directory.path().join("installed/swoop-cli")).unwrap(), b"new cli");
    assert_eq!(std::fs::read(directory.path().join("installed/user-file.txt")).unwrap(), b"user file");
    assert_eq!(backups.len(), 2);
    assert_eq!(std::fs::read(backups[0].join("Contents/MacOS/swoop")).unwrap(), b"old executable");
    assert_eq!(std::fs::read(&backups[1]).unwrap(), b"old cli");
}

#[test]
#[cfg(target_os = "macos")]
fn changed_installed_executable_or_archive_blocks_installation() {
    for change_executable in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let plan = plan(directory.path(), &good_archive());
        std::fs::write(if change_executable { &plan.executable } else { &plan.archive }, b"external edit").unwrap();
        assert!(swoop_updater::apply(&plan).is_err());
        assert_eq!(
            std::fs::read(&plan.executable).unwrap(),
            if change_executable { b"external edit".as_slice() } else { b"old executable".as_slice() }
        );
    }
}

#[test]
#[cfg(target_os = "macos")]
fn traversal_and_case_aliases_are_rejected_before_modifying_app() {
    for bytes in [archive(&[("../outside", b"evil")]), archive(&[("Swoop-test/a", b"a"), ("Swoop-test/A", b"b")])] {
        let directory = tempfile::tempdir().unwrap();
        let plan = plan(directory.path(), &bytes);
        assert!(swoop_updater::apply(&plan).is_err());
        assert_eq!(std::fs::read(&plan.executable).unwrap(), b"old executable");
        assert!(!directory.path().join("outside").exists());
    }
}

#[test]
#[cfg(target_os = "macos")]
fn changed_cli_or_bundle_resource_blocks_installation_before_any_move() {
    for relative in ["installed/swoop-cli", "installed/Swoop.app/Contents/Info.plist"] {
        let directory = tempfile::tempdir().unwrap();
        let plan = plan(directory.path(), &good_archive());
        let changed = directory.path().join(relative);
        std::fs::write(&changed, b"external resource edit").unwrap();
        assert!(swoop_updater::apply(&plan).is_err());
        assert_eq!(std::fs::read(&plan.executable).unwrap(), b"old executable");
        assert_eq!(std::fs::read(changed).unwrap(), b"external resource edit");
        assert!(!plan.journal_path.exists());
    }
}

#[test]
#[cfg(target_os = "macos")]
fn interrupted_publication_recovers_from_durable_journal_at_each_boundary() {
    for boundary in 0..3 {
        let directory = tempfile::tempdir().unwrap();
        let plan = plan(directory.path(), &good_archive());
        swoop_updater::apply(&plan).unwrap();
        let mut journal: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&plan.journal_path).unwrap()).unwrap();
        // Reproduce persisted Prepared state after the first backup move, first publication,
        // and final publication respectively. Files are real filesystem objects.
        journal["phase"] = "Prepared".into();
        if boundary < 2 {
            let cli = &journal["entries"][1];
            let destination = Path::new(cli["destination"].as_str().unwrap());
            let replacement = Path::new(cli["replacement"].as_str().unwrap());
            let backup = Path::new(cli["backup"].as_str().unwrap());
            std::fs::rename(destination, replacement).unwrap();
            std::fs::rename(backup, destination).unwrap();
        }
        if boundary == 0 {
            let bundle = &journal["entries"][0];
            std::fs::rename(
                Path::new(bundle["destination"].as_str().unwrap()),
                Path::new(bundle["replacement"].as_str().unwrap()),
            )
            .unwrap();
        }
        std::fs::write(&plan.journal_path, serde_json::to_vec(&journal).unwrap()).unwrap();
        assert!(swoop_updater::apply(&plan).unwrap_err().contains("已恢复原版本"));
        assert_eq!(std::fs::read(&plan.executable).unwrap(), b"old executable");
        assert_eq!(std::fs::read(directory.path().join("installed/swoop-cli")).unwrap(), b"old cli");
        let recovered: serde_json::Value = serde_json::from_slice(&std::fs::read(&plan.journal_path).unwrap()).unwrap();
        assert_eq!(recovered["phase"], "RolledBack");
    }
}

#[test]
#[cfg(target_os = "macos")]
fn recovery_keeps_external_replacement_and_retains_original_backup() {
    let directory = tempfile::tempdir().unwrap();
    let plan = plan(directory.path(), &good_archive());
    let backups = swoop_updater::apply(&plan).unwrap();
    let mut journal: serde_json::Value = serde_json::from_slice(&std::fs::read(&plan.journal_path).unwrap()).unwrap();
    journal["phase"] = "Prepared".into();
    std::fs::write(&plan.journal_path, serde_json::to_vec(&journal).unwrap()).unwrap();
    std::fs::write(&plan.executable, b"external user replacement").unwrap();
    assert!(swoop_updater::apply(&plan).unwrap_err().contains("人工恢复"));
    assert_eq!(std::fs::read(&plan.executable).unwrap(), b"external user replacement");
    assert_eq!(std::fs::read(backups[0].join("Contents/MacOS/swoop")).unwrap(), b"old executable");
    let recorded: serde_json::Value = serde_json::from_slice(&std::fs::read(&plan.journal_path).unwrap()).unwrap();
    assert_eq!(recorded["phase"], "RecoveryRequired");
}

#[test]
#[cfg(target_os = "macos")]
fn wrong_platform_binary_hash_and_reserved_archive_names_are_rejected() {
    for field in ["target=x86_64-pc-windows-msvc", "binary_sha256=0000", "application_id=other.app"] {
        let build = format!(
            "name=Swoop\napplication_id=com.muffin.swoop\nversion={}\ntarget={}-apple-darwin\nbinary_sha256={}\ncli_sha256={}\n",
            feed_version(),
            std::env::consts::ARCH,
            swoop_updater::hex(&Sha256::digest(b"new executable")),
            swoop_updater::hex(&Sha256::digest(b"new cli"))
        );
        let field_name = field.split_once('=').unwrap().0;
        let build = build
            .lines()
            .map(|line| if line.starts_with(&format!("{field_name}=")) { field } else { line })
            .collect::<Vec<_>>()
            .join("\n");
        let bytes = archive(&[
            ("Swoop-test/Swoop.app/Contents/MacOS/swoop", b"new executable"),
            ("Swoop-test/Swoop.app/Contents/Info.plist", b"new plist"),
            ("Swoop-test/swoop-cli", b"new cli"),
            ("Swoop-test/build-info.txt", build.as_bytes()),
        ]);
        let directory = tempfile::tempdir().unwrap();
        let plan = plan(directory.path(), &bytes);
        assert!(swoop_updater::apply(&plan).is_err());
        assert_eq!(std::fs::read(&plan.executable).unwrap(), b"old executable");
    }
    for name in ["Swoop-test/CON", "Swoop-test/file.", "Swoop-test/file ", "Swoop-test/a:stream", "Swoop-test/a\\b"] {
        let directory = tempfile::tempdir().unwrap();
        let plan = plan(directory.path(), &archive(&[(name, b"invalid")]));
        assert!(swoop_updater::apply(&plan).is_err(), "{name}");
        assert_eq!(std::fs::read(&plan.executable).unwrap(), b"old executable");
    }
}

#[test]
#[cfg(target_os = "macos")]
fn helper_restarts_verified_original_after_install_failure() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let executable = installation(directory.path());
    let script = b"#!/bin/sh\nprintf restarted > \"$SWOOP_DATA_DIR/restarted.txt\"\n";
    std::fs::write(&executable, script).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let bytes = good_archive();
    let archive = directory.path().join("update.zip");
    std::fs::write(&archive, &bytes).unwrap();
    let data = directory.path().join("data");
    std::fs::create_dir_all(data.join(".swoop-control")).unwrap();
    std::fs::write(data.join(".swoop-control/instance.lock"), b"").unwrap();
    let (plan_path, _, token) = swoop_updater::prepare(&candidate(&bytes), &archive, &executable, &data).unwrap();
    std::fs::write(&archive, b"changed after preparation").unwrap();
    assert!(swoop_updater::run_helper(&plan_path, &token).is_err());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !data.join("restarted.txt").exists() {
        assert!(std::time::Instant::now() < deadline, "verified original was not restarted");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(std::fs::read(&executable).unwrap(), script);
    assert_eq!(std::fs::read(data.join("restarted.txt")).unwrap(), b"restarted");
}

#[test]
#[cfg(target_os = "macos")]
fn committed_journal_is_idempotent_but_never_accepts_later_external_changes() {
    let directory = tempfile::tempdir().unwrap();
    let plan = plan(directory.path(), &good_archive());
    let backups = swoop_updater::apply(&plan).unwrap();
    assert_eq!(swoop_updater::apply(&plan).unwrap(), backups);
    std::fs::write(&plan.executable, b"external edit after completion").unwrap();
    assert!(swoop_updater::apply(&plan).is_err());
    assert_eq!(std::fs::read(&plan.executable).unwrap(), b"external edit after completion");
}

#[cfg(target_os = "macos")]
const OLD_SCRIPT: &[u8] = b"#!/bin/sh\nprintf old > \"$SWOOP_DATA_DIR/restarted.txt\"\n";
#[cfg(target_os = "macos")]
const NEW_SCRIPT: &[u8] = b"#!/bin/sh\nprintf new > \"$SWOOP_DATA_DIR/restarted.txt\"\n";

#[cfg(target_os = "macos")]
struct HelperFixture {
    _directory: tempfile::TempDir,
    plan_path: std::path::PathBuf,
    ready_path: std::path::PathBuf,
    plan: swoop_updater::InstallPlan,
    parent_lock: std::fs::File,
}
#[cfg(target_os = "macos")]
impl HelperFixture {
    fn new() -> Self {
        use fs2::FileExt;
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let executable = installation(directory.path());
        std::fs::write(&executable, OLD_SCRIPT).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let bytes = archive_with_executable(NEW_SCRIPT);
        let archive = directory.path().join("update.zip");
        std::fs::write(&archive, &bytes).unwrap();
        let data = directory.path().join("data");
        std::fs::create_dir_all(data.join(".swoop-control")).unwrap();
        let parent_lock = std::fs::OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .open(data.join(".swoop-control/instance.lock"))
            .unwrap();
        parent_lock.try_lock_exclusive().unwrap();
        let (plan_path, _, _) = swoop_updater::prepare(&candidate(&bytes), &archive, &executable, &data).unwrap();
        let plan = serde_json::from_slice(&std::fs::read(&plan_path).unwrap()).unwrap();
        let ready_path = plan_path.with_file_name("helper-ready.json");
        Self { _directory: directory, plan_path, ready_path, plan, parent_lock }
    }

    fn spawn(&self) -> std::thread::JoinHandle<swoop_updater::Result<()>> {
        let path = self.plan_path.clone();
        let token = self.plan.token.clone();
        let ready = self.ready_path.clone();
        std::thread::spawn(move || swoop_updater::run_helper_ready(&path, &token, &ready))
    }

    fn wait_ready(&self) {
        wait_until(|| {
            std::fs::read(&self.ready_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .is_some_and(|ready| ready["token"] == self.plan.token)
        });
    }

    fn wait_restarted(&self, marker: &[u8]) {
        wait_until(|| std::fs::read(self.plan.data_directory.join("restarted.txt")).is_ok_and(|value| value == marker));
    }

    fn report(&self) -> serde_json::Value {
        let entry = std::fs::read_dir(self.plan_path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap())
            .find(|entry| entry.file_name().to_string_lossy().starts_with("result-"))
            .unwrap();
        serde_json::from_slice(&std::fs::read(entry.path()).unwrap()).unwrap()
    }
}

#[cfg(target_os = "macos")]
fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !condition() {
        assert!(std::time::Instant::now() < deadline, "helper did not reach the expected state");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
#[cfg(target_os = "macos")]
fn helper_ready_waits_for_parent_lock_then_installs_and_restarts() {
    let fixture = HelperFixture::new();
    let helper = fixture.spawn();
    fixture.wait_ready();
    assert_eq!(std::fs::read(&fixture.plan.executable).unwrap(), OLD_SCRIPT);
    assert!(!fixture.plan.journal_path.exists());
    assert!(!fixture.plan.data_directory.join("restarted.txt").exists());
    assert!(!helper.is_finished());
    fs2::FileExt::unlock(&fixture.parent_lock).unwrap();
    helper.join().unwrap().unwrap();
    fixture.wait_restarted(b"new");
    assert_eq!(std::fs::read(&fixture.plan.executable).unwrap(), NEW_SCRIPT);
    assert_eq!(fixture.report()["installed"], true);
}

#[test]
#[cfg(target_os = "macos")]
fn helper_preflight_failures_never_announce_ready_or_replace_existing_ready() {
    for fault in [
        "token",
        "shape",
        "signature",
        "archive",
        "original",
        "missing-lock",
        "symlink-lock",
        "ready-name",
        "ready-directory",
        "ready-exists",
    ] {
        let mut fixture = HelperFixture::new();
        let mut token = fixture.plan.token.clone();
        let mut ready_path = fixture.ready_path.clone();
        match fault {
            "token" => token = uuid::Uuid::new_v4().to_string(),
            "shape" => fixture.plan.targets[0].name = "other.app".into(),
            "signature" => fixture.plan.candidate.feed.signature = "00".repeat(64),
            "archive" => std::fs::write(&fixture.plan.archive, b"changed archive").unwrap(),
            "original" => std::fs::write(&fixture.plan.executable, b"external edit").unwrap(),
            "missing-lock" => fixture.plan.data_directory = fixture.plan.data_directory.join("missing"),
            "symlink-lock" => {
                let new_data = fixture.plan.data_directory.join("other-data");
                std::fs::create_dir_all(new_data.join(".swoop-control")).unwrap();
                std::os::unix::fs::symlink(
                    fixture.plan.data_directory.join(".swoop-control/instance.lock"),
                    new_data.join(".swoop-control/instance.lock"),
                )
                .unwrap();
                fixture.plan.data_directory = new_data;
            }
            "ready-name" => ready_path = fixture.ready_path.with_file_name("other-ready.json"),
            "ready-directory" => ready_path = fixture.plan.data_directory.join("helper-ready.json"),
            "ready-exists" => std::fs::write(&fixture.ready_path, b"existing marker").unwrap(),
            _ => unreachable!(),
        }
        std::fs::write(&fixture.plan_path, serde_json::to_vec(&fixture.plan).unwrap()).unwrap();
        assert!(swoop_updater::run_helper_ready(&fixture.plan_path, &token, &ready_path).is_err(), "{fault}");
        if fault == "ready-exists" {
            assert_eq!(std::fs::read(&fixture.ready_path).unwrap(), b"existing marker");
        } else {
            assert!(!ready_path.exists(), "{fault}");
            assert!(!fixture.ready_path.exists(), "{fault}");
        }
        assert!(!fixture.plan.journal_path.exists(), "{fault}");
        assert!(!fixture.plan.data_directory.join("restarted.txt").exists(), "{fault}");
        let expected = if fault == "original" { b"external edit".as_slice() } else { OLD_SCRIPT };
        assert_eq!(std::fs::read(&fixture.plan.executable).unwrap(), expected, "{fault}");
    }
}

#[test]
#[cfg(target_os = "macos")]
fn helper_revalidates_after_parent_exit_and_restarts_unchanged_original_on_failure() {
    let fixture = HelperFixture::new();
    let helper = fixture.spawn();
    fixture.wait_ready();
    std::fs::write(&fixture.plan.archive, b"changed while parent was still running").unwrap();
    assert_eq!(std::fs::read(&fixture.plan.executable).unwrap(), OLD_SCRIPT);
    fs2::FileExt::unlock(&fixture.parent_lock).unwrap();
    assert!(helper.join().unwrap().unwrap_err().contains("更新包已改变"));
    fixture.wait_restarted(b"old");
    assert_eq!(std::fs::read(&fixture.plan.executable).unwrap(), OLD_SCRIPT);
    assert!(!fixture.plan.journal_path.exists());
    let report = fixture.report();
    assert_eq!(report["installed"], false);
    assert_eq!(report["original_restored"], true);
}

#[test]
#[cfg(target_os = "macos")]
fn legacy_helper_still_recovers_interrupted_journal_and_restarts_original() {
    let fixture = HelperFixture::new();
    fs2::FileExt::unlock(&fixture.parent_lock).unwrap();
    swoop_updater::apply(&fixture.plan).unwrap();
    let mut journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&fixture.plan.journal_path).unwrap()).unwrap();
    journal["phase"] = "Prepared".into();
    std::fs::write(&fixture.plan.journal_path, serde_json::to_vec(&journal).unwrap()).unwrap();
    assert!(swoop_updater::run_helper(&fixture.plan_path, &fixture.plan.token).unwrap_err().contains("已恢复原版本"));
    fixture.wait_restarted(b"old");
    assert_eq!(std::fs::read(&fixture.plan.executable).unwrap(), OLD_SCRIPT);
    assert!(!fixture.ready_path.exists());
    assert_eq!(fixture.report()["original_restored"], true);
}
