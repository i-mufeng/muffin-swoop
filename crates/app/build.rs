use std::{env, fs, path::PathBuf};

fn main() {
    const ICON: &str = "../../assets/brand/swoop/swoop.ico";
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={ICON}");
    // 版本号来自工作区根 Cargo.toml（version.workspace = true），两处都要监听。
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=../../Cargo.toml");

    // 按编译目标判断，保证 macOS/Linux 上交叉编译 Windows 时同样嵌入。
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let icon = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory")).join(ICON);
    assert!(icon.is_file(), "A 折光 ICO 母版必须存在");
    // 使用绝对路径和 UTF-8，支持检出目录包含空格或中文。
    let icon = icon.to_str().expect("ICO path must be UTF-8").replace('\\', "/");

    // 文件版本与 Cargo 版本保持一致（如 0.7.0 → 0,7,0,0），Windows 各处据此显示版本。
    let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let core = version.split(['-', '+']).next().expect("version core");
    let nums: Vec<u64> = core
        .split('.')
        .map(|part| part.parse().expect("numeric version component"))
        .collect();
    assert!(nums.len() == 3, "版本号必须是 major.minor.patch：{version}");
    let version4 = format!("{},{},{},0", nums[0], nums[1], nums[2]);

    // 缺少资源编译器或编译失败时直接失败，避免产出悄悄缺图标/版本信息的 EXE。
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));
    for (bin, description) in [("swoop", "Swoop"), ("swoop-cli", "Swoop CLI")] {
        let resource = out_dir.join(format!("{bin}-resources.rc"));
        fs::write(
            &resource,
            format!(
                r#"#pragma code_page(65001)
1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {version4}
PRODUCTVERSION {version4}
FILEFLAGSMASK 0x3F
FILEFLAGS 0x0
FILEOS 0x40004
FILETYPE 0x1
FILESUBTYPE 0x0
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName", "mufeng"
            VALUE "FileDescription", "{description}"
            VALUE "FileVersion", "{version}"
            VALUE "InternalName", "{bin}"
            VALUE "LegalCopyright", "Copyright (c) mufeng"
            VALUE "OriginalFilename", "{bin}.exe"
            VALUE "ProductName", "Swoop"
            VALUE "ProductVersion", "{version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 0x4B0
    END
END
"#
            ),
        )
        .expect("write Swoop version resource");
        embed_resource::compile_for(&resource, [bin], embed_resource::NONE)
            .manifest_required()
            .expect("failed to embed resources in Windows executables");
    }
}
