//! Opt-in compatibility test for the actual scripts/package-macos.sh ZIP.
//! Run with SWOOP_UPDATE_PACKAGE=/absolute/Swoop-....zip cargo test -p
//! swoop-updater --test package_smoke -- --ignored --nocapture.
//! An unchanged production updater is compiled as 0.0.0 in a temporary
//! workspace when the supplied package is the current version, so the normal
//! upgrade-only check stays enabled. No installed application is launched.
//! This proves package compatibility with the current production update code,
//! not an upgrade from a previously published application binary.
#![cfg(target_os = "macos")]

use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use swoop_updater::{Artifact, Channel, InstallPlan, Release, SignedFeed};

type Files = BTreeMap<String, (String, bool)>;

fn hash(mut input: impl Read) -> String {
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = input.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    swoop_updater::hex(&digest.finalize())
}
fn file_hash(path: &Path) -> String {
    hash(File::open(path).unwrap())
}
fn bundle_files(root: &Path) -> Files {
    fn visit(root: &Path, directory: &Path, files: &mut Files) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let metadata = fs::symlink_metadata(entry.path()).unwrap();
            assert!(
                !metadata.file_type().is_symlink(),
                "unexpected installed symbolic link"
            );
            if metadata.is_dir() {
                visit(root, &entry.path(), files);
            } else {
                assert!(metadata.is_file());
                files.insert(
                    entry
                        .path()
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    (
                        file_hash(&entry.path()),
                        metadata.permissions().mode() & 0o111 != 0,
                    ),
                );
            }
        }
    }
    let mut files = Files::new();
    visit(root, root, &mut files);
    files
}
fn archive_manifest(path: &Path) -> (BTreeMap<String, String>, Files, (String, bool)) {
    let mut archive =
        zip::ZipArchive::new(File::open(path).unwrap()).expect("actual package must be ZIP");
    let build_names = archive
        .file_names()
        .filter(|name| !name.starts_with("__MACOSX/") && name.ends_with("/build-info.txt"))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(
        build_names.len(),
        1,
        "package must have exactly one build-info.txt"
    );
    let (root, _) = build_names[0].rsplit_once('/').unwrap();
    assert!(!root.contains('/') && root.starts_with("Swoop-"));
    let mut text = String::new();
    archive
        .by_name(&build_names[0])
        .unwrap()
        .take(16_385)
        .read_to_string(&mut text)
        .unwrap();
    assert!(text.len() <= 16_384);
    let mut fields = BTreeMap::new();
    for line in text.lines().filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once('=').expect("invalid build-info field");
        assert!(fields.insert(name.to_owned(), value.to_owned()).is_none());
    }
    assert_eq!(fields["application_id"], "com.muffin.swoop");
    assert_eq!(
        fields["target"],
        format!("{}-apple-darwin", std::env::consts::ARCH)
    );
    let prefix = format!("{root}/Swoop.app/");
    let mut files = Files::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        if entry.is_dir() {
            continue;
        }
        if let Some(relative) = entry.name().strip_prefix(&prefix) {
            let relative = relative.to_owned();
            let executable = entry.unix_mode().unwrap_or(0) & 0o111 != 0;
            assert!(
                files
                    .insert(relative, (hash(&mut entry), executable))
                    .is_none()
            );
        }
    }
    let mut cli = archive
        .by_name(&format!("{root}/swoop-cli"))
        .expect("standalone CLI must be packaged");
    let executable = cli.unix_mode().unwrap_or(0) & 0o111 != 0;
    let cli = (hash(&mut cli), executable);
    assert_eq!(files["Contents/MacOS/swoop"].0, fields["binary_sha256"]);
    assert!(files["Contents/MacOS/swoop"].1 && cli.1);
    assert_eq!(cli.0, fields["cli_sha256"]);
    assert!(files.contains_key("Contents/Info.plist"));
    assert!(files.contains_key("Contents/Resources/FONT-NOTICE.md"));
    (fields, files, cli)
}

fn run_as_old_version(package: &Path) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repository = source.parent().unwrap().parent().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let member = temp.path().join("updater");
    fs::create_dir(&member).unwrap();
    fs::create_dir(member.join("tests")).unwrap();
    // Reuse the exact production source, dependency declarations and workspace
    // configuration. Only the simulated installed version differs.
    fs::copy(source.join("Cargo.toml"), member.join("Cargo.toml")).unwrap();
    std::os::unix::fs::symlink(source.join("src"), member.join("src")).unwrap();
    std::os::unix::fs::symlink(
        source.join("tests/package_smoke.rs"),
        member.join("tests/package_smoke.rs"),
    )
    .unwrap();
    let workspace = fs::read_to_string(repository.join("Cargo.toml")).unwrap();
    let package_section = workspace.find("[workspace.package]").unwrap();
    let workspace = workspace[package_section..].replacen(
        &format!("version = \"{}\"", env!("CARGO_PKG_VERSION")),
        "version = \"0.0.0\"",
        1,
    );
    assert!(workspace.starts_with("[workspace.package]\nversion = \"0.0.0\""));
    fs::write(
        temp.path().join("Cargo.toml"),
        format!("[workspace]\nresolver = \"3\"\nmembers = [\"updater\"]\n\n{workspace}"),
    )
    .unwrap();
    // Seed dependency resolution from the real workspace lock and stay offline.
    // The isolated lock may prune unrelated packages and alter only this local
    // package version; it never writes to the repository's Cargo.lock.
    let lock = fs::read_to_string(repository.join("Cargo.lock"))
        .unwrap()
        .replacen(
            &format!(
                "name = \"swoop-updater\"\nversion = \"{}\"",
                env!("CARGO_PKG_VERSION")
            ),
            "name = \"swoop-updater\"\nversion = \"0.0.0\"",
            1,
        );
    fs::write(temp.path().join("Cargo.lock"), lock).unwrap();
    // Keep dependency artifacts between smoke runs, in a separate Cargo target
    // directory so the nested build cannot deadlock on its parent's build lock.
    // All installation targets, data and signing material still live in tempdirs.
    let cache = repository.join("target/updater-package-smoke");
    let status = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["test", "--offline", "--manifest-path"])
        .arg(temp.path().join("Cargo.toml"))
        .args(["--target-dir"])
        .arg(cache)
        .args([
            "-p",
            "swoop-updater",
            "--test",
            "package_smoke",
            "--",
            "--ignored",
            "--nocapture",
        ])
        .env("SWOOP_UPDATE_PACKAGE", package)
        .env("SWOOP_UPDATE_PACKAGE_OLD_HOST", "1")
        .status()
        .expect("run isolated old-version updater test");
    assert!(
        status.success(),
        "isolated old-version updater package test failed: {status}"
    );
}

#[test]
#[ignore = "requires an actual macOS release ZIP via SWOOP_UPDATE_PACKAGE; uses temporary install targets only"]
fn packaged_macos_zip_installs_exact_bundle_and_cli_with_signed_feed_and_preserved_originals() {
    let package = PathBuf::from(
        std::env::var_os("SWOOP_UPDATE_PACKAGE")
            .expect("set SWOOP_UPDATE_PACKAGE to the real release ZIP"),
    );
    let package = fs::canonicalize(package).expect("release ZIP must exist");
    assert!(fs::symlink_metadata(&package).unwrap().is_file());
    let (build, expected_bundle, expected_cli) = archive_manifest(&package);
    let release_version = semver::Version::parse(&build["version"]).unwrap();
    let host_version = semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
    if release_version <= host_version {
        assert!(
            std::env::var_os("SWOOP_UPDATE_PACKAGE_OLD_HOST").is_none(),
            "release must be newer than the isolated old host"
        );
        run_as_old_version(&package);
        return;
    }
    let original_archive_hash = file_hash(&package);
    let temporary = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temporary.path()).unwrap();
    let archive = root.join("release.zip");
    fs::copy(&package, &archive).unwrap();
    assert_eq!(file_hash(&archive), original_archive_hash);
    let bundle = root.join("installed/Swoop.app");
    let executable = bundle.join("Contents/MacOS/swoop");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::create_dir_all(bundle.join("Contents/Resources")).unwrap();
    fs::write(&executable, b"old installed GUI executable").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(bundle.join("Contents/Info.plist"), b"old installed plist").unwrap();
    fs::write(
        bundle.join("Contents/Resources/old-resource.txt"),
        b"old resource retained in backup",
    )
    .unwrap();
    let cli = root.join("installed/swoop-cli");
    fs::write(&cli, b"old installed standalone CLI").unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
    let old_bundle = bundle_files(&bundle);
    let old_cli_hash = file_hash(&cli);
    let user_file = root.join("installed/user-file.txt");
    fs::write(&user_file, b"unrelated user document").unwrap();
    let data = root.join("user-data");
    fs::create_dir_all(data.join("downloads")).unwrap();
    fs::write(
        data.join("downloads/existing.bin"),
        b"existing download data",
    )
    .unwrap();
    fs::write(data.join("settings.json"), b"{\"user\":\"settings\"}").unwrap();
    let user_data_before = bundle_files(&data);

    // Public deterministic test seed; never read a developer/production key.
    let seed_file = root.join("public-test-seed.bin");
    fs::write(&seed_file, [31_u8; 32]).unwrap();
    fs::set_permissions(&seed_file, fs::Permissions::from_mode(0o600)).unwrap();
    let seed: [u8; 32] = fs::read(seed_file).unwrap().try_into().unwrap();
    let key = SigningKey::from_bytes(&seed);
    let release = Release {
        version: build["version"].clone(),
        channel: Channel::Stable,
        notes: "Local package compatibility smoke test".into(),
        platforms: BTreeMap::from([(
            swoop_updater::platform(),
            Artifact {
                url: "https://release-package.test/Swoop.zip".into(),
                size: fs::metadata(&archive).unwrap().len(),
                sha256: original_archive_hash.clone(),
            },
        )]),
    };
    // Same production envelope as examples/sign_feed.rs: sign the exact UTF-8
    // Release JSON and retain it as the SignedFeed payload, without reserializing.
    let payload = serde_json::to_string(&release).unwrap();
    let feed = SignedFeed {
        signature: swoop_updater::hex(&key.sign(payload.as_bytes()).to_bytes()),
        payload,
    };
    let public_key = swoop_updater::hex(&key.verifying_key().to_bytes());
    let feed_file = root.join("feed.json");
    fs::write(&feed_file, serde_json::to_vec(&feed).unwrap()).unwrap();
    let persisted_feed = serde_json::from_slice(&fs::read(feed_file).unwrap()).unwrap();
    let candidate = swoop_updater::verify(
        &persisted_feed,
        &public_key,
        env!("CARGO_PKG_VERSION"),
        &Channel::Stable,
        &swoop_updater::platform(),
    )
    .unwrap()
    .expect("signed release must be an upgrade for this host");
    let (plan_path, helper, token) =
        swoop_updater::prepare(&candidate, &archive, &executable, &data).unwrap();
    assert!(plan_path.starts_with(&root) && helper.starts_with(&root));
    assert_eq!(file_hash(&helper), old_bundle["Contents/MacOS/swoop"].0);
    let plan: InstallPlan = serde_json::from_slice(&fs::read(&plan_path).unwrap()).unwrap();
    assert_eq!(plan.token, token);
    assert_eq!(plan.targets.len(), 2);
    assert!(
        plan.targets
            .iter()
            .all(|target| target.destination.starts_with(&root))
    );
    let backups = swoop_updater::apply(&plan).unwrap();
    assert_eq!(
        bundle_files(&bundle),
        expected_bundle,
        "all installed bundle files and executable modes must match the actual ZIP"
    );
    assert_eq!(
        (
            file_hash(&cli),
            fs::metadata(&cli).unwrap().permissions().mode() & 0o111 != 0
        ),
        expected_cli
    );
    assert_eq!(file_hash(&executable), build["binary_sha256"]);
    assert_eq!(backups.len(), 2);
    for (backup, target) in backups.iter().zip(&plan.targets) {
        assert!(backup.starts_with(&root));
        match target.name.as_str() {
            "Swoop.app" => assert_eq!(bundle_files(backup), old_bundle),
            "swoop-cli" => assert_eq!(file_hash(backup), old_cli_hash),
            other => panic!("unexpected installation target: {other}"),
        }
    }
    assert_eq!(fs::read(&user_file).unwrap(), b"unrelated user document");
    assert_eq!(bundle_files(&data), user_data_before);
    assert_eq!(
        file_hash(&package),
        original_archive_hash,
        "input release ZIP must stay unchanged"
    );
    assert_eq!(
        swoop_updater::apply(&plan).unwrap(),
        backups,
        "reapplying a committed plan must be idempotent"
    );
    let bundle_hash = hash(serde_json::to_vec(&expected_bundle).unwrap().as_slice());
    println!(
        "Verified actual package {}: version={}, archive_sha256={}, bundle_manifest_sha256={}, gui_sha256={}, cli_sha256={}, backups=2, user_files_preserved=true",
        package.display(),
        build["version"],
        original_archive_hash,
        bundle_hash,
        build["binary_sha256"],
        build["cli_sha256"]
    );
}
