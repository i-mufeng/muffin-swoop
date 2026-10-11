use std::{env, fs, path::PathBuf};

fn main() {
    const ICON: &str = "../../assets/brand/swoop/swoop.ico";
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={ICON}");

    // 按编译目标判断，保证 macOS/Linux 上交叉编译 Windows 时同样嵌入。
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let icon = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory")).join(ICON);
    assert!(icon.is_file(), "A 折光 ICO 母版必须存在");
    // 使用绝对路径和 UTF-8，支持检出目录包含空格或中文。
    let icon = icon.to_str().expect("ICO path must be UTF-8").replace('\\', "/");
    let resource = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory")).join("swoop-icon.rc");
    fs::write(&resource, format!("#pragma code_page(65001)\n1 ICON \"{icon}\"\n")).expect("write Swoop icon resource");

    // 缺少资源编译器或图标编译失败时直接失败，避免产出悄悄缺图标的 EXE。
    embed_resource::compile_for(resource, ["swoop", "swoop-cli"], embed_resource::NONE)
        .manifest_required()
        .expect("failed to embed A 折光 in Windows executables");
}
