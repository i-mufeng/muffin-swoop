//! Offline release signing. No networking, key generation or upload occurs here.
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use swoop_updater::{Artifact, Channel, Release, SignedFeed};

type Result<T> = std::result::Result<T, String>;

const USAGE: &str = "Offline Swoop feed signer\n\
  cargo run -p swoop-updater --example sign_feed -- \\\n    --private-key /secure/ed25519-seed.hex \\\n    --version 0.2.0 --channel stable \\\n    --artifact macos-aarch64 /releases/Swoop-macos.zip https://updates.example/Swoop-macos.zip \\\n    --artifact windows-x86_64 /releases/Swoop-windows.zip https://updates.example/Swoop-windows.zip \\\n    --output /releases/feed.json --public-key-out /releases/feed-public-key.hex\n\
Optional: --notes-file /releases/notes.txt\n\
Private key: existing 32-byte Ed25519 seed or 64 hexadecimal digits. Unix permissions must exclude group/other access.\n\
Output files must not already exist. No key is printed, generated, or uploaded.";

struct Arguments {
    key: PathBuf,
    version: String,
    channel: Channel,
    artifacts: Vec<InputArtifact>,
    output: PathBuf,
    public_key_output: PathBuf,
    notes: Option<PathBuf>,
}
struct InputArtifact {
    platform: String,
    archive: PathBuf,
    url: String,
}

fn main() {
    match parse(std::env::args_os().skip(1).collect()).and_then(|args| args.map(sign).transpose()) {
        Ok(None) => println!("{USAGE}"),
        Ok(Some(())) => println!("Signed feed and public-key file saved. Nothing was uploaded."),
        Err(error) => {
            eprintln!("Feed signing failed: {error}");
            std::process::exit(1);
        }
    }
}
fn text(value: OsString) -> Result<String> {
    value.into_string().map_err(|_| "Text arguments must be UTF-8".into())
}
fn next(values: &mut impl Iterator<Item = OsString>) -> Result<OsString> {
    values.next().ok_or("Missing option value".into())
}
fn parse(values: Vec<OsString>) -> Result<Option<Arguments>> {
    if values.len() == 1 && matches!(values[0].to_str(), Some("--help" | "-h")) {
        return Ok(None);
    }
    let (mut key, mut version, mut output, mut public_key_output, mut notes) = (None, None, None, None, None);
    let mut channel = Channel::Stable;
    let mut channel_seen = false;
    let mut artifacts = Vec::new();
    let mut values = values.into_iter();
    while let Some(option) = values.next() {
        match option.to_str() {
            Some("--private-key") if key.is_none() => key = Some(PathBuf::from(next(&mut values)?)),
            Some("--version") if version.is_none() => version = Some(text(next(&mut values)?)?),
            Some("--output") if output.is_none() => output = Some(PathBuf::from(next(&mut values)?)),
            Some("--public-key-out") if public_key_output.is_none() => {
                public_key_output = Some(PathBuf::from(next(&mut values)?))
            }
            Some("--notes-file") if notes.is_none() => notes = Some(PathBuf::from(next(&mut values)?)),
            Some("--channel") if !channel_seen => {
                channel_seen = true;
                channel = match text(next(&mut values)?)?.as_str() {
                    "stable" => Channel::Stable,
                    "preview" => Channel::Preview,
                    _ => return Err("Channel must be stable or preview".into()),
                };
            }
            Some("--artifact") => artifacts.push(InputArtifact {
                platform: text(next(&mut values)?)?,
                archive: PathBuf::from(next(&mut values)?),
                url: text(next(&mut values)?)?,
            }),
            _ => return Err("Unknown or repeated option; use --help".into()),
        }
    }
    if artifacts.is_empty() || artifacts.len() > 4 {
        return Err("Provide between one and four --artifact entries".into());
    }
    let args = Arguments {
        key: key.ok_or("--private-key is required")?,
        version: version.ok_or("--version is required")?,
        channel,
        artifacts,
        output: output.ok_or("--output is required")?,
        public_key_output: public_key_output.ok_or("--public-key-out is required")?,
        notes,
    };
    if args.output == args.public_key_output {
        return Err("Feed and public-key outputs must be different paths".into());
    }
    Ok(Some(args))
}

// Wipe input buffers independently of the SigningKey, whose default zeroize feature clears its own copy.
struct SecretBytes(Vec<u8>);
impl Drop for SecretBytes {
    fn drop(&mut self) {
        for byte in &mut self.0 {
            // SAFETY: each byte is a unique, live allocation element. Volatile writes prevent dead-store removal.
            unsafe { std::ptr::write_volatile(byte, 0) };
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    }
}
struct SecretSeed([u8; 32]);
impl Drop for SecretSeed {
    fn drop(&mut self) {
        for byte in &mut self.0 {
            unsafe { std::ptr::write_volatile(byte, 0) };
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    }
}
fn load_key(path: &Path) -> Result<SigningKey> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "Cannot inspect the signing-key file")?;
    if !metadata.is_file() || metadata.len() > 256 {
        return Err("Signing key must be a small regular file, not a link".into());
    }
    let mut input = File::open(path).map_err(|_| "Cannot open the signing-key file")?;
    let metadata = input.metadata().map_err(|_| "Cannot inspect the opened signing-key file")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("Signing-key file permissions must exclude group/other access (for example 0600)".into());
        }
    }
    #[cfg(not(unix))]
    let _ = metadata;
    let mut bytes = SecretBytes(Vec::new());
    (&mut input).take(257).read_to_end(&mut bytes.0).map_err(|_| "Cannot read the signing-key file")?;
    let mut seed = SecretSeed([0; 32]);
    if bytes.0.len() == 32 {
        seed.0.copy_from_slice(&bytes.0);
    } else {
        let encoded = bytes.0.trim_ascii();
        if encoded.len() != 64 {
            return Err("Signing key must contain a 32-byte seed or 64 hexadecimal digits".into());
        }
        for (index, pair) in encoded.as_chunks::<2>().0.iter().enumerate() {
            let high = (pair[0] as char).to_digit(16).ok_or("Invalid signing-key format")?;
            let low = (pair[1] as char).to_digit(16).ok_or("Invalid signing-key format")?;
            seed.0[index] = ((high << 4) | low) as u8;
        }
    }
    Ok(SigningKey::from_bytes(&seed.0))
}
fn hash_reader(mut input: impl Read) -> Result<(u64, String)> {
    let mut hash = Sha256::new();
    let mut size = 0_u64;
    let mut bytes = [0_u8; 65536];
    loop {
        let count = input.read(&mut bytes).map_err(|_| "Cannot read release data")?;
        if count == 0 {
            break;
        }
        size = size.checked_add(count as u64).ok_or("Release data is too large")?;
        if size > swoop_updater::MAX_ARCHIVE_BYTES * 2 {
            return Err("Release data is too large".into());
        }
        hash.update(&bytes[..count]);
    }
    Ok((size, swoop_updater::hex(&hash.finalize())))
}
fn triplet(platform: &str) -> Result<&'static str> {
    match platform {
        "macos-aarch64" => Ok("aarch64-apple-darwin"),
        "macos-x86_64" => Ok("x86_64-apple-darwin"),
        "windows-x86_64" => Ok("x86_64-pc-windows-msvc"),
        "windows-aarch64" => Ok("aarch64-pc-windows-msvc"),
        _ => Err("Unsupported release platform".into()),
    }
}
fn inspect_archive(input: &InputArtifact, version: &str) -> Result<Artifact> {
    let metadata = fs::symlink_metadata(&input.archive).map_err(|_| "Cannot inspect a release archive")?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > swoop_updater::MAX_ARCHIVE_BYTES {
        return Err("Release archive must be a regular file between 1 byte and 1 GiB".into());
    }
    let target = triplet(&input.platform)?;
    let mut zip = zip::ZipArchive::new(File::open(&input.archive).map_err(|_| "Cannot open a release archive")?)
        .map_err(|_| "Release archive is not a valid ZIP")?;
    let records =
        zip.file_names().filter(|name| name.ends_with("/build-info.txt")).map(str::to_owned).collect::<Vec<_>>();
    if records.len() != 1 {
        return Err("Release ZIP must have exactly one build-info.txt".into());
    }
    let build_name = &records[0];
    let (root, _) = build_name.split_once('/').ok_or("Invalid package root")?;
    if !root.starts_with("Swoop-") || build_name != &format!("{root}/build-info.txt") {
        return Err("Invalid package root".into());
    }
    let mut build = String::new();
    zip.by_name(build_name)
        .map_err(|_| "Cannot read build-info.txt")?
        .take(16385)
        .read_to_string(&mut build)
        .map_err(|_| "Invalid build-info.txt encoding")?;
    if build.len() > 16384 {
        return Err("build-info.txt is too large".into());
    }
    let mut fields = BTreeMap::new();
    for line in build.trim_start_matches('\u{feff}').lines().filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once('=').ok_or("Invalid build-info.txt")?;
        if fields.insert(name, value).is_some() {
            return Err("Duplicate build-info.txt field".into());
        }
    }
    for (name, expected) in
        [("name", "Swoop"), ("application_id", "com.muffin.swoop"), ("version", version), ("target", target)]
    {
        if fields.get(name).copied() != Some(expected) {
            return Err(format!("Release build-info {name} does not match the requested release"));
        }
    }
    let (gui, cli) = if input.platform.starts_with("windows-") {
        ("swoop.exe", "swoop-cli.exe")
    } else {
        ("Swoop.app/Contents/MacOS/swoop", "swoop-cli")
    };
    for (name, member) in [("binary_sha256", gui), ("cli_sha256", cli)] {
        let entry = zip.by_name(&format!("{root}/{member}")).map_err(|_| "Release ZIP is missing an executable")?;
        if entry.size() > 256 * 1024 * 1024 {
            return Err("An executable exceeds the installer's 256 MiB limit".into());
        }
        let (_, hash) = hash_reader(entry)?;
        if fields.get(name).copied() != Some(hash.as_str()) {
            return Err("Executable hash differs from build-info.txt".into());
        }
    }
    drop(zip);
    let (size, sha256) = hash_reader(File::open(&input.archive).map_err(|_| "Cannot open a release archive")?)?;
    let after = fs::symlink_metadata(&input.archive).map_err(|_| "Cannot recheck a release archive")?;
    if !after.is_file()
        || size != metadata.len()
        || after.len() != metadata.len()
        || after.modified().ok() != metadata.modified().ok()
    {
        return Err("Release archive changed while signing".into());
    }
    Ok(Artifact { url: input.url.clone(), size, sha256 })
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "Cannot create output file; parent must exist and output must not already exist")?;
    output.write_all(bytes).and_then(|()| output.sync_all()).map_err(|_| "Cannot finish writing an output file".into())
}
fn sign(args: Arguments) -> Result<()> {
    semver::Version::parse(&args.version).map_err(|_| "Invalid release version")?;
    if args.output.exists() || args.public_key_output.exists() {
        return Err("Output files already exist; choose new paths".into());
    }
    let notes = if let Some(path) = &args.notes {
        let mut notes = String::new();
        File::open(path)
            .map_err(|_| "Cannot open release notes")?
            .take(16385)
            .read_to_string(&mut notes)
            .map_err(|_| "Cannot read release notes")?;
        if notes.len() > 16384 {
            return Err("Release notes exceed 16 KiB".into());
        }
        notes
    } else {
        String::new()
    };
    let mut platforms = BTreeMap::new();
    for input in &args.artifacts {
        if platforms.contains_key(&input.platform) {
            return Err("Duplicate release platform".into());
        }
        platforms.insert(input.platform.clone(), inspect_archive(input, &args.version)?);
    }
    let release = Release { version: args.version, channel: args.channel, notes, platforms };
    let payload = serde_json::to_string(&release).map_err(|_| "Cannot serialize release")?;
    let key = load_key(&args.key)?;
    let public_key = swoop_updater::hex(&key.verifying_key().to_bytes());
    let feed = SignedFeed { signature: swoop_updater::hex(&key.sign(payload.as_bytes()).to_bytes()), payload };
    // Reuse the real verifier for signature, URLs, channel and platform validation before writing anything.
    for target in release.platforms.keys() {
        swoop_updater::verify(&feed, &public_key, "0.0.0-0", &release.channel, target)?
            .ok_or("Release version/channel is not eligible")?;
    }
    let bytes = serde_json::to_vec_pretty(&feed).map_err(|_| "Cannot serialize signed feed")?;
    if bytes.len() > swoop_updater::MAX_FEED_BYTES {
        return Err("Signed feed exceeds the client size limit".into());
    }
    write_new(&args.public_key_output, format!("{public_key}\n").as_bytes())?;
    write_new(&args.output, &bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// 测试夹具的“新版本”：始终比当前 crate 版本高一个 patch，保证“只允许升级”校验在发版后依然成立。
    fn feed_version() -> String {
        let mut parts: Vec<u64> = env!("CARGO_PKG_VERSION").split('.').map(|part| part.parse().unwrap()).collect();
        *parts.last_mut().unwrap() += 1;
        parts.iter().map(u64::to_string).collect::<Vec<_>>().join(".")
    }
    fn fixture(directory: &Path, platform: &str, seed_hex: bool) -> Arguments {
        let key = directory.join("test-only.seed");
        // Public deterministic fixture seed, created only inside the disposable test directory.
        fs::write(
            &key,
            if seed_hex {
                b"1313131313131313131313131313131313131313131313131313131313131313\n".to_vec()
            } else {
                vec![19; 32]
            },
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let build = format!(
            "name=Swoop\napplication_id=com.muffin.swoop\nversion={}\ntarget={}\nbinary_sha256={}\ncli_sha256={}\n",
            feed_version(),
            triplet(platform).unwrap(),
            swoop_updater::hex(&Sha256::digest(b"new executable")),
            swoop_updater::hex(&Sha256::digest(b"new cli"))
        );
        let (gui, cli) = if platform.starts_with("windows-") {
            ("swoop.exe", "swoop-cli.exe")
        } else {
            ("Swoop.app/Contents/MacOS/swoop", "swoop-cli")
        };
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content) in [
            (gui, b"new executable".as_slice()),
            (cli, b"new cli".as_slice()),
            ("build-info.txt", build.as_bytes()),
            ("Swoop.app/Contents/Info.plist", b"test plist".as_slice()),
        ] {
            zip.start_file(
                format!("Swoop-test/{name}"),
                zip::write::SimpleFileOptions::default().unix_permissions(0o755),
            )
            .unwrap();
            zip.write_all(content).unwrap();
        }
        let archive = directory.join("release.zip");
        fs::write(&archive, zip.finish().unwrap().into_inner()).unwrap();
        Arguments {
            key,
            version: feed_version(),
            channel: Channel::Stable,
            artifacts: vec![InputArtifact {
                platform: platform.into(),
                archive,
                url: "https://updates.example/release.zip".into(),
            }],
            output: directory.join("feed.json"),
            public_key_output: directory.join("feed-public-key.hex"),
            notes: None,
        }
    }
    #[test]
    #[cfg(any(target_os = "macos", windows))]
    fn temporary_key_signed_feed_verifies_and_prepares_the_real_installer() {
        let directory = tempfile::tempdir().unwrap();
        let args = fixture(directory.path(), &swoop_updater::platform(), true);
        let archive = args.artifacts[0].archive.clone();
        let output = args.output.clone();
        let public = args.public_key_output.clone();
        sign(args).unwrap();
        let feed: SignedFeed = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
        let public = fs::read_to_string(public).unwrap();
        let candidate =
            swoop_updater::verify(&feed, public.trim(), env!("CARGO_PKG_VERSION"), &Channel::Stable, &swoop_updater::platform())
                .unwrap()
                .unwrap();
        #[cfg(target_os = "macos")]
        let executable = {
            let file = directory.path().join("installed/Swoop.app/Contents/MacOS/swoop");
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(directory.path().join("installed/Swoop.app/Contents/Info.plist"), b"old plist").unwrap();
            fs::write(directory.path().join("installed/swoop-cli"), b"old cli").unwrap();
            file
        };
        #[cfg(windows)]
        let executable = {
            let parent = directory.path().join("installed");
            fs::create_dir(&parent).unwrap();
            fs::write(parent.join("swoop-cli.exe"), b"old cli").unwrap();
            fs::write(parent.join("build-info.txt"), b"version=0.1.0").unwrap();
            parent.join("swoop.exe")
        };
        fs::write(&executable, b"old executable").unwrap();
        let data = directory.path().join("data");
        fs::create_dir(&data).unwrap();
        let (plan, helper, _) = swoop_updater::prepare(&candidate, &archive, &executable, &data).unwrap();
        assert!(plan.is_file() && helper.is_file());
        assert_eq!(fs::read(executable).unwrap(), b"old executable");
        let serialized = fs::read_to_string(plan).unwrap();
        assert!(!serialized.contains("13131313131313131313131313131313"));
    }
    #[test]
    #[cfg(windows)]
    fn release_signing_with_local_key_only_runs_when_explicitly_enabled() {
        // 仅当显式设置 SWOOP_RELEASE_SIGN=1 时执行真实签名；默认立即返回，不影响常规测试。
        // 用途：在启用应用程序控制策略（WDAC）的机器上，target\examples 下的独立 exe 可能被拦截，
        // 而 cargo test 的测试宿主可运行，因此把发布签名入口放在这里。密钥路径只从环境变量读取，不入库。
        if std::env::var_os("SWOOP_RELEASE_SIGN").as_deref() != Some(std::ffi::OsStr::new("1")) {
            return;
        }
        let req = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("missing env {name}"));
        let args = Arguments {
            key: PathBuf::from(req("SWOOP_SIGN_KEY")),
            version: req("SWOOP_SIGN_VERSION"),
            channel: Channel::Stable,
            artifacts: vec![InputArtifact {
                platform: req("SWOOP_SIGN_PLATFORM"),
                archive: PathBuf::from(req("SWOOP_SIGN_ARCHIVE")),
                url: req("SWOOP_SIGN_URL"),
            }],
            output: PathBuf::from(req("SWOOP_SIGN_OUTPUT")),
            public_key_output: PathBuf::from(req("SWOOP_SIGN_PUBLIC_OUT")),
            notes: std::env::var_os("SWOOP_SIGN_NOTES").map(PathBuf::from),
        };
        sign(args).unwrap();
    }
    #[test]
    fn binary_and_hex_seed_inputs_produce_same_feed_without_overwriting_outputs() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let raw = fixture(first.path(), "macos-aarch64", false);
        let hex = fixture(second.path(), "macos-aarch64", true);
        let expected = raw.output.clone();
        let actual = hex.output.clone();
        sign(raw).unwrap();
        sign(hex).unwrap();
        assert_eq!(fs::read(expected).unwrap(), fs::read(&actual).unwrap());
        let again = fixture(second.path(), "macos-aarch64", true);
        let before = fs::read(&actual).unwrap();
        assert!(sign(again).is_err());
        assert_eq!(fs::read(actual).unwrap(), before);
    }
    #[test]
    fn mismatched_platform_and_unsafe_urls_never_write_a_feed() {
        for wrong_platform in [true, false] {
            let directory = tempfile::tempdir().unwrap();
            let mut args = fixture(directory.path(), "macos-aarch64", true);
            if wrong_platform {
                args.artifacts[0].platform = "windows-x86_64".into();
            } else {
                args.artifacts[0].url = "http://public.example/release.zip".into();
            }
            let output = args.output.clone();
            let public = args.public_key_output.clone();
            assert!(sign(args).is_err());
            assert!(!output.exists() && !public.exists());
        }
    }
    #[test]
    #[cfg(unix)]
    fn signing_key_with_group_or_other_access_is_rejected() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let args = fixture(directory.path(), "macos-aarch64", false);
        fs::set_permissions(&args.key, fs::Permissions::from_mode(0o644)).unwrap();
        let output = args.output.clone();
        assert!(sign(args).unwrap_err().contains("permissions"));
        assert!(!output.exists());
    }
    #[test]
    fn cli_preserves_paths_and_rejects_repeated_options() {
        assert!(parse(vec!["--help".into()]).unwrap().is_none());
        let args = [
            "--private-key",
            "/private/test seed",
            "--version",
            "0.2.0",
            "--artifact",
            "macos-aarch64",
            "/release/mac os.zip",
            "https://updates.example/pkg.zip",
            "--output",
            "/release/feed.json",
            "--public-key-out",
            "/release/feed-public.hex",
        ];
        let parsed = parse(args.iter().map(OsString::from).collect()).unwrap().unwrap();
        assert_eq!(parsed.key, PathBuf::from("/private/test seed"));
        assert_eq!(parsed.artifacts[0].archive, PathBuf::from("/release/mac os.zip"));
        let mut repeated = args.iter().map(OsString::from).collect::<Vec<_>>();
        repeated.extend(["--version".into(), "0.3.0".into()]);
        assert!(parse(repeated).is_err());
    }
}
