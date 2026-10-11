use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Candidate, Result, file_hash, platform, verify};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallPlan {
    pub candidate: Candidate,
    pub archive: PathBuf,
    pub executable: PathBuf,
    pub data_directory: PathBuf,
    pub current_sha256: String,
    pub token: String,
    pub journal_path: PathBuf,
    pub targets: Vec<InstallTarget>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallTarget {
    pub name: String,
    pub destination: PathBuf,
    pub fingerprint: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    token: String,
    phase: Phase,
    entries: Vec<JournalEntry>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    Prepared,
    Committed,
    RolledBack,
    RecoveryRequired,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalEntry {
    destination: PathBuf,
    replacement: PathBuf,
    backup: PathBuf,
    old_hash: String,
    new_hash: String,
}

pub fn prepare(
    candidate: &Candidate,
    archive: &Path,
    executable: &Path,
    data: &Path,
) -> Result<(PathBuf, PathBuf, String)> {
    let executable = fs::canonicalize(executable).map_err(|e| e.to_string())?;
    let destinations = install_destinations(&executable)?;
    let archive = fs::canonicalize(archive).map_err(|e| e.to_string())?;
    let directory = archive.parent().ok_or("更新包目录无效")?.join(format!("install-{}", uuid::Uuid::new_v4()));
    private_directory(&directory)?;
    let token = uuid::Uuid::new_v4().to_string();
    let targets = destinations
        .into_iter()
        .map(|(name, destination)| {
            let fingerprint = tree_hash(&destination)?;
            Ok(InstallTarget { name, destination, fingerprint })
        })
        .collect::<Result<Vec<_>>>()?;
    let plan = InstallPlan {
        candidate: candidate.clone(),
        archive,
        executable: executable.clone(),
        data_directory: fs::canonicalize(data).map_err(|e| e.to_string())?,
        current_sha256: file_hash(&executable)?,
        token: token.clone(),
        journal_path: directory.join("journal.json"),
        targets,
    };
    validate_plan(&plan)?;
    let path = directory.join("plan.json");
    write_new_json(&path, &plan)?;
    let helper = directory.join(if cfg!(windows) { "swoop-update-helper.exe" } else { "swoop-update-helper" });
    copy_tree(&executable, &helper)?;
    if file_hash(&helper)? != plan.current_sha256 {
        return Err("复制更新助手期间应用发生变化".into());
    }
    sync_directory(&directory)?;
    Ok((path, helper, token))
}

fn trusted_candidate(plan: &InstallPlan) -> Result<Candidate> {
    verify(
        &plan.candidate.feed,
        &plan.candidate.public_key,
        env!("CARGO_PKG_VERSION"),
        &plan.candidate.release.channel,
        &platform(),
    )?
    .ok_or("此更新已不适用".into())
}
fn validate_shape(plan: &InstallPlan) -> Result<()> {
    if uuid::Uuid::parse_str(&plan.token).is_err()
        || !plan.journal_path.is_absolute()
        || plan.journal_path.file_name().and_then(|name| name.to_str()) != Some("journal.json")
    {
        return Err("更新计划路径或授权无效".into());
    }
    let destinations =
        expected_destinations(&plan.executable, plan.targets.iter().any(|target| target.name == "swoop-cli"))?;
    if destinations.len() != plan.targets.len() {
        return Err("更新目标清单无效".into());
    }
    for ((name, destination), target) in destinations.iter().zip(&plan.targets) {
        if *name != target.name || *destination != target.destination || target.fingerprint.len() != 64 {
            return Err("更新目标清单与安装路径不一致".into());
        }
    }
    Ok(())
}
fn validate_plan(plan: &InstallPlan) -> Result<Candidate> {
    validate_shape(plan)?;
    let candidate = trusted_candidate(plan)?;
    let metadata = fs::symlink_metadata(&plan.archive).map_err(|e| e.to_string())?;
    if !metadata.is_file()
        || metadata.len() != candidate.artifact.size
        || file_hash(&plan.archive)? != candidate.artifact.sha256.to_ascii_lowercase()
    {
        return Err("待安装更新包已改变".into());
    }
    validate_originals(plan)?;
    Ok(candidate)
}
fn validate_originals(plan: &InstallPlan) -> Result<()> {
    for target in &plan.targets {
        if tree_hash(&target.destination)? != target.fingerprint {
            return Err(format!("安装文件 {} 已被其他程序修改，请重新检查更新", target.name));
        }
    }
    if file_hash(&plan.executable)? != plan.current_sha256 {
        return Err("应用已被其他程序更新，请重新检查更新".into());
    }
    Ok(())
}

fn install_destinations(executable: &Path) -> Result<Vec<(String, PathBuf)>> {
    let targets = expected_destinations(
        executable,
        executable
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .and_then(Path::parent)
            .is_some_and(|parent| parent.join("swoop-cli").is_file()),
    )?;
    #[cfg(target_os = "macos")]
    if !targets[0].1.join("Contents/Info.plist").is_file() {
        return Err("请从完整 Swoop.app 安装包中更新".into());
    }
    #[cfg(windows)]
    if !targets.iter().all(|(_, target)| target.is_file()) {
        return Err("请从完整 Windows 安装包中更新".into());
    }
    Ok(targets)
}
fn expected_destinations(executable: &Path, with_cli: bool) -> Result<Vec<(String, PathBuf)>> {
    if !executable.is_absolute()
        || executable.components().any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err("应用安装路径必须为规范绝对路径".into());
    }
    #[cfg(target_os = "macos")]
    {
        let macos = executable.parent().ok_or("应用路径无效")?;
        let contents = macos.parent().ok_or("应用路径无效")?;
        let bundle = contents.parent().ok_or("应用路径无效")?;
        if executable.file_name().and_then(|s| s.to_str()) != Some("swoop")
            || macos.file_name().and_then(|s| s.to_str()) != Some("MacOS")
            || contents.file_name().and_then(|s| s.to_str()) != Some("Contents")
            || bundle.file_name().and_then(|s| s.to_str()) != Some("Swoop.app")
        {
            return Err("请从完整 Swoop.app 安装包中更新".into());
        }
        let mut targets = vec![("Swoop.app".into(), bundle.into())];
        if with_cli {
            targets.push(("swoop-cli".into(), bundle.parent().ok_or("应用路径无效")?.join("swoop-cli")));
        }
        Ok(targets)
    }
    #[cfg(windows)]
    {
        let _ = with_cli;
        let parent = executable.parent().ok_or("应用路径无效")?;
        if executable.file_name().and_then(|s| s.to_str()) != Some("swoop.exe") {
            return Err("Windows 应用名称应为 swoop.exe".into());
        }
        Ok(vec![
            ("swoop.exe".into(), executable.into()),
            ("swoop-cli.exe".into(), parent.join("swoop-cli.exe")),
            ("build-info.txt".into(), parent.join("build-info.txt")),
        ])
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = with_cli;
        Err("此平台尚未提供安装器".into())
    }
}

fn portable_name(name: &str) -> bool {
    if name.is_empty()
        || name.ends_with(['.', ' '])
        || name.contains(['\\', ':', '<', '>', '"', '|', '?', '*'])
        || name.chars().any(|c| c.is_control())
    {
        return false;
    }
    let stem = name.split('.').next().unwrap().to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !(stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}
fn extract(archive: &Path, staging: &Path) -> Result<PathBuf> {
    let mut zip =
        zip::ZipArchive::new(File::open(archive).map_err(|e| e.to_string())?).map_err(|_| "更新包 ZIP 格式无效")?;
    if zip.len() > 4096 {
        return Err("更新包文件数量过多".into());
    }
    let mut seen = HashSet::new();
    let mut prefixes = BTreeMap::new();
    let mut root = None;
    let mut total = 0_u64;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|_| "无法读取 ZIP 项目")?;
        let enclosed = entry.enclosed_name().ok_or("更新包包含越界路径")?;
        if enclosed
            .components()
            .any(|part| !matches!(part, Component::Normal(_)) || !portable_name(&part.as_os_str().to_string_lossy()))
        {
            return Err("更新包路径无效或含系统保留名称".into());
        }
        if enclosed.components().next().is_some_and(|c| c.as_os_str() == "__MACOSX") {
            continue;
        }
        let first = enclosed.components().next().ok_or("更新包路径为空")?.as_os_str().to_owned();
        if !first.to_string_lossy().starts_with("Swoop-") {
            return Err("更新包目录名称无效".into());
        }
        if let Some(previous) = &root {
            if previous != &first {
                return Err("更新包有多个根目录".into());
            }
        } else {
            root = Some(first);
        }
        if !seen.insert(enclosed.to_string_lossy().trim_end_matches('/').to_lowercase()) {
            return Err("更新包包含重复或不兼容路径".into());
        }
        let mut prefix = PathBuf::new();
        for part in enclosed.components() {
            prefix.push(part.as_os_str());
            let name = prefix.to_string_lossy().into_owned();
            if prefixes.insert(name.to_lowercase(), name.clone()).is_some_and(|previous| previous != name) {
                return Err("更新包目录含大小写别名".into());
            }
        }
        let kind = entry.unix_mode().unwrap_or(0) & 0o170000;
        if !matches!(kind, 0 | 0o100000 | 0o040000) {
            return Err("更新包不允许符号链接或特殊文件".into());
        }
        total = total.checked_add(entry.size()).ok_or("更新包过大")?;
        if total > 2 * crate::MAX_ARCHIVE_BYTES || entry.size() > 256 * 1024 * 1024 {
            return Err("更新包解压大小超限".into());
        }
        let path = staging.join(&enclosed);
        if entry.is_dir() {
            if entry.size() != 0 {
                return Err("更新包目录不能含文件内容".into());
            }
            fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            continue;
        }
        fs::create_dir_all(path.parent().ok_or("更新包路径无效")?).map_err(|e| e.to_string())?;
        let mode = entry.unix_mode();
        let expected = entry.size();
        let mut output = OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| e.to_string())?;
        let copied = std::io::copy(&mut entry.by_ref().take(expected + 1), &mut output).map_err(|e| e.to_string())?;
        if copied != expected {
            return Err("更新包解压长度错误".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                &path,
                fs::Permissions::from_mode(if mode.unwrap_or(0) & 0o111 != 0 { 0o755 } else { 0o644 }),
            )
            .map_err(|e| e.to_string())?;
        }
        #[cfg(not(unix))]
        let _ = mode;
        output.sync_all().map_err(|e| e.to_string())?;
    }
    Ok(staging.join(root.ok_or("更新包为空")?))
}

fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source).map_err(|e| e.to_string())?;
    if metadata.is_dir() {
        fs::create_dir(target).map_err(|e| e.to_string())?;
        for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            copy_tree(&entry.path(), &target.join(entry.file_name()))?;
        }
        sync_directory(target)?;
    } else if metadata.is_file() {
        let mut input = File::open(source).map_err(|e| e.to_string())?;
        let mut output = OpenOptions::new().write(true).create_new(true).open(target).map_err(|e| e.to_string())?;
        std::io::copy(&mut input, &mut output).map_err(|e| e.to_string())?;
        fs::set_permissions(target, metadata.permissions()).map_err(|e| e.to_string())?;
        output.sync_all().map_err(|e| e.to_string())?;
    } else {
        return Err("更新文件不是普通文件".into());
    }
    Ok(())
}
fn tree_hash(path: &Path) -> Result<String> {
    fn visit(path: &Path, hash: &mut Sha256) -> Result<()> {
        let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if metadata.is_file() {
            hash.update(b"file\0");
            hash.update(file_hash(path)?.as_bytes());
        } else if metadata.is_dir() {
            hash.update(b"directory\0");
            let mut children = fs::read_dir(path)
                .map_err(|e| e.to_string())?
                .collect::<std::io::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?;
            children.sort_by_key(|entry| entry.file_name());
            for entry in children {
                let name = entry.file_name();
                let bytes = name.as_encoded_bytes();
                hash.update((bytes.len() as u64).to_le_bytes());
                hash.update(bytes);
                visit(&entry.path(), hash)?;
            }
        } else {
            return Err("安装内容含符号链接或特殊文件".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            hash.update(metadata.permissions().mode().to_le_bytes());
        }
        Ok(())
    }
    let mut hash = Sha256::new();
    visit(path, &mut hash)?;
    Ok(crate::hex(&hash.finalize()))
}
fn validate_build(source: &Path, candidate: &Candidate) -> Result<()> {
    let text = fs::read_to_string(source.join("build-info.txt")).map_err(|_| "更新包缺少构建信息")?;
    if text.len() > 16384 {
        return Err("构建信息过大".into());
    }
    let mut fields = BTreeMap::new();
    for line in text.trim_start_matches('\u{feff}').lines().filter(|line| !line.trim().is_empty()) {
        let (name, value) = line.split_once('=').ok_or("构建信息格式无效")?;
        if fields.insert(name, value).is_some() {
            return Err("构建信息字段重复".into());
        }
    }
    let target = match std::env::consts::OS {
        "macos" => format!("{}-apple-darwin", std::env::consts::ARCH),
        "windows" => format!("{}-pc-windows-msvc", std::env::consts::ARCH),
        _ => return Err("更新平台不受支持".into()),
    };
    for (key, expected) in [
        ("name", "Swoop"),
        ("application_id", "com.muffin.swoop"),
        ("version", candidate.release.version.as_str()),
        ("target", target.as_str()),
    ] {
        if fields.get(key).copied() != Some(expected) {
            return Err(format!("安装包 {key} 与签名版本/当前平台不同"));
        }
    }
    let (gui, cli) = if cfg!(windows) {
        (source.join("swoop.exe"), source.join("swoop-cli.exe"))
    } else {
        (source.join("Swoop.app/Contents/MacOS/swoop"), source.join("swoop-cli"))
    };
    for (name, file) in [("binary_sha256", gui), ("cli_sha256", cli)] {
        if fields.get(name).copied() != Some(file_hash(&file)?.as_str()) {
            return Err(format!("安装包 {name} 与实际文件不同"));
        }
    }
    Ok(())
}

/// A prepared journal is durable before any installed path is moved. Recovery only restores
/// exact recorded originals and never overwrites an externally changed destination.
pub fn apply(plan: &InstallPlan) -> Result<Vec<PathBuf>> {
    validate_shape(plan)?;
    if plan.journal_path.exists() {
        let mut journal = read_journal(plan)?;
        if journal.phase == Phase::Committed {
            for entry in &journal.entries {
                if tree_hash(&entry.destination)? != entry.new_hash {
                    return Err("已安装的新版本又被外部修改，已保留当前文件".into());
                }
            }
            return Ok(journal.entries.iter().map(|entry| entry.backup.clone()).collect());
        }
        recover_journal(plan, &mut journal)?;
        return Err("上次中断的更新已恢复原版本，请重新检查更新".into());
    }
    let candidate = validate_plan(plan)?;
    let staging = plan.journal_path.parent().ok_or("更新目录无效")?.join(format!("unpack-{}", uuid::Uuid::new_v4()));
    private_directory(&staging)?;
    // Snapshot then hash the bytes actually extracted, closing the archive path replacement window.
    let snapshot = staging.join("verified.zip");
    copy_tree(&plan.archive, &snapshot)?;
    if fs::metadata(&snapshot).map_err(|e| e.to_string())?.len() != candidate.artifact.size
        || file_hash(&snapshot)? != candidate.artifact.sha256.to_ascii_lowercase()
    {
        return Err("复制更新包期间内容发生变化".into());
    }
    let source = extract(&snapshot, &staging)?;
    validate_build(&source, &candidate)?;
    let mut entries = Vec::new();
    for (index, target) in plan.targets.iter().enumerate() {
        let parent = target.destination.parent().ok_or("应用目录无效")?;
        let replacement = parent.join(format!(".swoop-new-{}-{index}", plan.token));
        let backup = parent.join(format!(".swoop-backup-{}-{index}", plan.token));
        copy_tree(&source.join(&target.name), &replacement)?;
        let new_hash = tree_hash(&replacement)?;
        entries.push(JournalEntry {
            destination: target.destination.clone(),
            replacement,
            backup,
            old_hash: target.fingerprint.clone(),
            new_hash,
        });
        sync_directory(parent)?;
    }
    validate_originals(plan)?;
    let mut journal = Journal { token: plan.token.clone(), phase: Phase::Prepared, entries };
    write_new_json(&plan.journal_path, &journal)?;
    let result = publish(&journal);
    if let Err(error) = result {
        return match recover_journal(plan, &mut journal) {
            Ok(()) => Err(format!("更新安装失败，已恢复原版本：{error}")),
            Err(recovery) => {
                Err(format!("更新安装失败：{error}；{recovery}；恢复记录：{}", plan.journal_path.display()))
            }
        };
    }
    journal.phase = Phase::Committed;
    if let Err(error) = replace_journal(&plan.journal_path, &journal) {
        return match recover_journal(plan, &mut journal) {
            Ok(()) => Err(format!("无法记录安装完成，已恢复原版本：{error}")),
            Err(recovery) => Err(format!("无法记录安装完成：{error}；{recovery}")),
        };
    }
    Ok(journal.entries.into_iter().map(|entry| entry.backup).collect())
}
fn publish(journal: &Journal) -> Result<()> {
    for entry in &journal.entries {
        move_exclusive(&entry.destination, &entry.backup)?;
        if tree_hash(&entry.backup)? != entry.old_hash {
            return Err("安装原件在替换前被外部修改，未发布新版".into());
        }
        if tree_hash(&entry.replacement)? != entry.new_hash {
            return Err("准备安装的新文件发生变化".into());
        }
        move_exclusive(&entry.replacement, &entry.destination)?;
    }
    Ok(())
}
fn read_journal(plan: &InstallPlan) -> Result<Journal> {
    let journal: Journal = read_json(&plan.journal_path)?;
    if journal.token != plan.token || journal.entries.len() != plan.targets.len() {
        return Err("更新恢复记录与授权不匹配".into());
    }
    for (index, (entry, target)) in journal.entries.iter().zip(&plan.targets).enumerate() {
        let parent = target.destination.parent().ok_or("恢复目录无效")?;
        if entry.destination != target.destination
            || entry.old_hash != target.fingerprint
            || entry.backup != parent.join(format!(".swoop-backup-{}-{index}", plan.token))
            || entry.replacement != parent.join(format!(".swoop-new-{}-{index}", plan.token))
        {
            return Err("更新恢复路径无效".into());
        }
    }
    Ok(journal)
}
fn recover_journal(plan: &InstallPlan, journal: &mut Journal) -> Result<()> {
    let mut errors = Vec::new();
    for entry in journal.entries.iter().rev() {
        if let Err(error) = restore_entry(entry) {
            errors.push(error);
        }
    }
    journal.phase = if errors.is_empty() { Phase::RolledBack } else { Phase::RecoveryRequired };
    replace_journal(&plan.journal_path, journal)?;
    if errors.is_empty() { Ok(()) } else { Err(format!("部分原件需人工恢复：{}", errors.join("；"))) }
}
fn restore_entry(entry: &JournalEntry) -> Result<()> {
    if !entry.backup.try_exists().map_err(|e| e.to_string())? {
        return if tree_hash(&entry.destination)? == entry.old_hash {
            Ok(())
        } else {
            Err(format!("{} 已有外部改动，已保留", entry.destination.display()))
        };
    }
    let backup_hash = tree_hash(&entry.backup)?;
    if entry.destination.try_exists().map_err(|e| e.to_string())? {
        let current = tree_hash(&entry.destination)?;
        if current == entry.old_hash && backup_hash == entry.old_hash {
            return Ok(());
        }
        if current != entry.new_hash {
            return Err(format!("{} 已有外部改动，原件保留在 {}", entry.destination.display(), entry.backup.display()));
        }
        let retained = entry.destination.with_file_name(format!(".swoop-rollback-{}", uuid::Uuid::new_v4()));
        move_exclusive(&entry.destination, &retained)?;
        // Recheck the item actually moved; an external replacement is retained, not deleted.
        if tree_hash(&retained)? != entry.new_hash {
            let _ = move_exclusive(&retained, &entry.destination);
            return Err(format!("安装目标在恢复前变化，文件保留在 {}", retained.display()));
        }
    }
    // Even an externally edited original moved just before publication must be put back intact.
    // Its unexpected hash prevents committing the update; no content is discarded.
    move_exclusive(&entry.backup, &entry.destination)?;
    Ok(())
}

fn private_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(path).map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    fs::create_dir(path).map_err(|e| e.to_string())?;
    Ok(())
}
fn write_new_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path).map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec(value).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    sync_directory(path.parent().ok_or("记录目录无效")?)
}
fn replace_journal(path: &Path, value: &Journal) -> Result<()> {
    let temporary = path.with_file_name(format!("journal-{}.tmp", uuid::Uuid::new_v4()));
    write_new_json(&temporary, value)?;
    fs::rename(&temporary, path).map_err(|e| e.to_string())?;
    sync_directory(path.parent().ok_or("记录目录无效")?)
}
fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    if !fs::symlink_metadata(path).map_err(|e| e.to_string())?.is_file() {
        return Err("更新记录不是普通文件".into());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take((crate::MAX_FEED_BYTES * 4 + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > crate::MAX_FEED_BYTES * 4 {
        return Err("更新记录过大".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "更新记录格式无效".into())
}
fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path).and_then(|file| file.sync_all()).map_err(|e| e.to_string())?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn move_exclusive(source: &Path, destination: &Path) -> Result<()> {
    rename_exclusive(source, destination).map_err(|e| e.to_string())?;
    sync_directory(destination.parent().ok_or("移动目录无效")?)
}
#[cfg(target_os = "macos")]
fn rename_exclusive(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        fn renamex_np(from: *const std::ffi::c_char, to: *const std::ffi::c_char, flags: u32) -> i32;
    }
    let from = std::ffi::CString::new(source.as_os_str().as_bytes())?;
    let to = std::ffi::CString::new(destination.as_os_str().as_bytes())?;
    if unsafe { renamex_np(from.as_ptr(), to.as_ptr(), 0x0000_0004) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
#[cfg(windows)]
fn rename_exclusive(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    }
    let from: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = destination.as_os_str().encode_wide().chain(Some(0)).collect();
    // WRITE_THROUGH; deliberately omit REPLACE_EXISTING.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0x8) } != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
#[cfg(not(any(target_os = "macos", windows)))]
fn rename_exclusive(_: &Path, _: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "unsupported install platform"))
}

pub fn run_helper(plan_path: &Path, token: &str) -> Result<()> {
    run_helper_inner(plan_path, token, None)
}

/// Announce successful startup before waiting for the parent application's instance lock.
/// The caller must keep that lock until it reads the matching token from the ready file.
pub fn run_helper_ready(plan_path: &Path, token: &str, ready_path: &Path) -> Result<()> {
    run_helper_inner(plan_path, token, Some(ready_path))
}

fn run_helper_inner(plan_path: &Path, token: &str, ready_path: Option<&Path>) -> Result<()> {
    let plan: InstallPlan = read_json(plan_path)?;
    if plan.token != token || uuid::Uuid::parse_str(token).is_err() || plan_path.parent() != plan.journal_path.parent()
    {
        return Err("更新安装授权无效".into());
    }
    validate_shape(&plan)?;
    if let Some(ready_path) = ready_path {
        if ready_path != plan_path.with_file_name("helper-ready.json") {
            return Err("更新助手就绪记录必须位于计划目录并命名为 helper-ready.json".into());
        }
        // New installations must pass the full preflight while the GUI is still alive.
        // The legacy recovery entry point intentionally permits partially moved originals.
        validate_plan(&plan)?;
    }
    let lock_path = plan.data_directory.join(".swoop-control/instance.lock");
    if !fs::symlink_metadata(&lock_path).map_err(|e| e.to_string())?.is_file() {
        return Err("实例锁无效".into());
    }
    let lock = OpenOptions::new().read(true).write(true).open(lock_path).map_err(|e| e.to_string())?;
    let mut acquired = try_instance_lock(&lock)?;
    if let Some(ready_path) = ready_path {
        write_new_json(ready_path, &serde_json::json!({"token":token}))?;
    }
    let deadline = Instant::now() + Duration::from_secs(120);
    while !acquired {
        if Instant::now() >= deadline {
            return Err("应用没有在两分钟内退出，未安装更新".into());
        }
        std::thread::sleep(Duration::from_millis(200));
        acquired = try_instance_lock(&lock)?;
    }
    let result = apply(&plan);
    let installed = result.is_ok();
    let restart_safe = installed || validate_originals(&plan).is_ok();
    let report = match &result {
        Ok(backups) => {
            serde_json::json!({"version":plan.candidate.release.version,"backups":backups,"installed":true,"journal":plan.journal_path})
        }
        Err(error) => {
            serde_json::json!({"installed":false,"error":error,"journal":plan.journal_path,"original_restored":restart_safe})
        }
    };
    let report_result =
        write_new_json(&plan_path.with_file_name(format!("result-{}.json", uuid::Uuid::new_v4())), &report);
    FileExt::unlock(&lock).map_err(|e| e.to_string())?;
    drop(lock);
    if restart_safe {
        std::process::Command::new(&plan.executable)
            .env("SWOOP_DATA_DIR", &plan.data_directory)
            .spawn()
            .map_err(|e| format!("更新处理结束，重启失败：{e}；恢复记录：{}", plan.journal_path.display()))?;
    }
    result?;
    report_result?;
    Ok(())
}

fn try_instance_lock(lock: &File) -> Result<bool> {
    match lock.try_lock_exclusive() {
        Ok(()) => Ok(true),
        Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => Ok(false),
        Err(error) => Err(format!("无法锁定应用实例：{error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(any(target_os = "macos", windows))]
    fn publication_rename_never_replaces_existing_file_or_directory() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::write(&source, b"new version").unwrap();
        fs::write(&destination, b"external user file").unwrap();
        assert!(move_exclusive(&source, &destination).is_err());
        assert_eq!(fs::read(&source).unwrap(), b"new version");
        assert_eq!(fs::read(&destination).unwrap(), b"external user file");
        let source = temp.path().join("source-dir");
        let destination = temp.path().join("destination-dir");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&destination).unwrap();
        assert!(move_exclusive(&source, &destination).is_err());
        assert!(source.is_dir() && destination.is_dir());
    }
    #[test]
    fn portable_zip_names_reject_windows_aliases() {
        for name in ["CON", "con.txt", "LPT1", "COM9.log", "a.", "a ", "a:b", "a\\b", ".."] {
            assert!(!portable_name(name), "{name}");
        }
        assert!(portable_name("Swoop.app"));
        assert!(portable_name("COM10.txt"));
    }
}
