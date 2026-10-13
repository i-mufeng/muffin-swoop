use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// 定位写入：各 worker 共享同一个 File，无需 seek，也无需加锁。
pub fn write_at(file: &File, buf: &[u8], pos: u64) -> (usize, io::Result<()>) {
    let mut written = 0usize;
    while written < buf.len() {
        #[cfg(unix)]
        let result = {
            use std::os::unix::fs::FileExt;
            file.write_at(&buf[written..], pos + written as u64)
        };
        #[cfg(windows)]
        let result = {
            use std::os::windows::fs::FileExt;
            file.seek_write(&buf[written..], pos + written as u64)
        };
        match result {
            Ok(0) => {
                return (written, Err(io::Error::new(io::ErrorKind::WriteZero, "write returned 0")));
            }
            Ok(n) => written += n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return (written, Err(error)),
        }
    }
    (written, Ok(()))
}

/// 常见文件系统单个文件名上限 255 字节（UTF-16 的 NTFS 是 255 个单元），留出 " (n)" 去重后缀和 `.swoop` 的余量
const MAX_FILE_NAME_BYTES: usize = 240;

/// 文件名去掉路径分隔符等危险字符，过长时截断主名、保留扩展名。
pub fn sanitize_file_name(name: &str) -> String {
    let name = truncate_file_name(name);
    let name = name.as_str();
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if trimmed.is_empty() {
        "download".to_string()
    } else {
        // Windows device names remain reserved even when followed by an extension.
        let stem = trimmed.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
        let numbered_device = stem.strip_prefix("COM").or_else(|| stem.strip_prefix("LPT")).is_some_and(|suffix| {
            matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
        });
        if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") || numbered_device {
            format!("_{trimmed}")
        } else {
            trimmed.to_string()
        }
    }
}

fn truncate_file_name(name: &str) -> String {
    if name.len() <= MAX_FILE_NAME_BYTES {
        return name.to_string();
    }
    let ext = match name.rfind('.') {
        Some(i) if i > 0 && name.len() - i <= 16 => &name[i..],
        _ => "",
    };
    let mut cut = MAX_FILE_NAME_BYTES - ext.len();
    while !name.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{ext}", name[..cut].trim_end())
}

/// 未完成下载的数据写在目标旁的 `<文件名>.swoop`，完成后再改名为目标文件名。
pub const PART_SUFFIX: &str = ".swoop";

pub fn part_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(PART_SUFFIX);
    PathBuf::from(name)
}

/// 同名顺延编号的上限
pub const MAX_NUMBERED: u32 = 10_000;

/// 第 n 个去重名称：0 为原名，其余为 `主名 (n).扩展名`。
pub fn numbered_name(name: &str, n: u32) -> String {
    if n == 0 {
        return name.to_owned();
    }
    match name.rfind('.') {
        Some(i) if i > 0 => format!("{} ({n}){}", &name[..i], &name[i..]),
        _ => format!("{name} ({n})"),
    }
}

/// 为新下载占用名称：目标（含普通文件、目录、断链）不存在，且以 create_new 原子创建
/// `<名称>.swoop` 成功；任一被占用就顺延 " (n)" 编号。返回的文件是临时文件。
pub fn reserve_file(
    dir: &Path,
    name: &str,
    registry: &crate::TargetRegistry,
) -> io::Result<(String, File, crate::TargetLease)> {
    for n in 0..MAX_NUMBERED {
        let candidate = numbered_name(name, n);
        let target = dir.join(&candidate);
        let lease = match registry.reserve(&target) {
            Ok(lease) => lease,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
            Err(error) => return Err(error),
        };
        match std::fs::symlink_metadata(&target) {
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        match OpenOptions::new().create_new(true).read(true).write(true).open(part_path(&target)) {
            Ok(file) => return Ok((candidate, file, lease)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "同名文件过多，请更换下载文件名"))
}

/// 不覆盖地改名；目标已存在返回 `AlreadyExists`，源文件保持原样。
pub fn rename_noclobber(from: &Path, to: &Path) -> io::Result<()> {
    let error = match tempfile::TempPath::try_from_path(from)?.persist_noclobber(to) {
        Ok(()) => return Ok(()),
        Err(error) => {
            // TempPath 析构会删文件，失败时必须交还所有权。
            let _ = error.path.keep();
            error.error
        }
    };
    if error.kind() == io::ErrorKind::AlreadyExists {
        return Err(error);
    }
    // exFAT 等不支持原子不覆盖改名（也不支持硬链接）的文件系统：先确认目标不存在再普通改名。
    match std::fs::symlink_metadata(to) {
        Ok(_) => Err(io::Error::new(io::ErrorKind::AlreadyExists, "目标文件已存在")),
        Err(missing) if missing.kind() == io::ErrorKind::NotFound => std::fs::rename(from, to),
        Err(other) => Err(other),
    }
}

/// 续传只能打开已存在的普通文件；不创建零洞文件，也不跟随目标符号链接。
pub fn open_resume_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "续传目标不能是链接"));
        }
    }
    if !metadata.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "续传目标不是普通文件"));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    #[test]
    fn long_names_keep_extension() {
        let name = format!("{}.jpg", "壁".repeat(200));
        let cleaned = super::sanitize_file_name(&name);
        assert!(cleaned.len() <= super::MAX_FILE_NAME_BYTES, "{}", cleaned.len());
        assert!(cleaned.ends_with("壁.jpg"));
        assert_eq!(super::sanitize_file_name("a.zip"), "a.zip");
    }

    use super::*;

    #[test]
    fn names_are_portable_to_windows_without_changing_normal_unicode_names() {
        for name in ["CON", "con.txt", "NUL.tar.gz", "COM1.txt", "lpt³.log", "AUX. "] {
            assert!(sanitize_file_name(name).starts_with('_'), "{name}");
        }
        assert_eq!(sanitize_file_name(" 报告 中文.pdf. . "), "报告 中文.pdf");
        assert_eq!(sanitize_file_name("COM10.txt"), "COM10.txt");
        assert_eq!(sanitize_file_name("../bad\\name:stream"), "_bad_name_stream");
    }

    #[cfg(unix)]
    #[test]
    fn dangling_links_are_occupied_and_resume_does_not_follow_links() {
        let dir = tempfile::tempdir().unwrap();
        let external = dir.path().join("missing.bin");
        std::os::unix::fs::symlink(&external, dir.path().join("file.bin")).unwrap();
        let (name, _, _lease) = reserve_file(dir.path(), "file.bin", &crate::TargetRegistry::default()).unwrap();
        assert_eq!(name, "file (1).bin");
        assert!(dir.path().join("file (1).bin.swoop").is_file());
        assert!(!dir.path().join("file (1).bin").exists());
        assert!(!external.exists());
        std::fs::write(&external, b"preserve").unwrap();
        assert!(open_resume_file(&dir.path().join("file.bin")).is_err());
        assert_eq!(std::fs::read(external).unwrap(), b"preserve");
    }

    #[test]
    fn reserve_skips_part_files_of_other_tasks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file.bin.swoop"), b"other task").unwrap();
        let (name, _, _lease) = reserve_file(dir.path(), "file.bin", &crate::TargetRegistry::default()).unwrap();
        assert_eq!(name, "file (1).bin");
        assert_eq!(std::fs::read(dir.path().join("file.bin.swoop")).unwrap(), b"other task");
    }

    #[test]
    fn rename_noclobber_keeps_both_files_on_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("a.swoop"), dir.path().join("a"));
        std::fs::write(&from, b"new").unwrap();
        std::fs::write(&to, b"user").unwrap();
        assert_eq!(super::rename_noclobber(&from, &to).unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&from).unwrap(), b"new");
        assert_eq!(std::fs::read(&to).unwrap(), b"user");
        std::fs::remove_file(&to).unwrap();
        super::rename_noclobber(&from, &to).unwrap();
        assert!(!from.exists());
        assert_eq!(std::fs::read(&to).unwrap(), b"new");
    }
}
