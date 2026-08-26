//! Windows：在 HKCU 下注册“按用户”字体（仅 Windows 平台编译）。
//!
//! 需要 Windows 10 1809 或更新版本；更早版本会忽略 HKCU 下的字体注册。

use std::path::Path;

use winreg::enums::*;
use winreg::RegKey;

const FONTS_KEY: &str = r"Software\Microsoft\Windows NT\CurrentVersion\Fonts";

/// 注册表值名格式: "字体名 (TrueType)" 或 "字体名 (OpenType)"，
/// 值为字体文件的完整路径。
pub fn register_font(
    dest: &Path,
    family: &str,
    is_otf: bool,
    dry_run: bool,
    verbose: bool,
) -> std::io::Result<()> {
    let kind = if is_otf { "OpenType" } else { "TrueType" };
    let value_name = format!("{} ({})", family, kind);
    let value = dest.to_string_lossy().into_owned();

    if dry_run {
        println!("[dry-run] 注册: {} = {}", value_name, value);
        return Ok(());
    }

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _disp) = hkcu.create_subkey(FONTS_KEY)?;

    if key.get_value::<String, _>(&value_name).is_ok() {
        eprintln!("警告: 注册表条目已存在，将覆盖: {}", value_name);
    }

    key.set_value(&value_name, &value)?;

    if verbose {
        println!("注册: {} = {}", value_name, value);
    }
    Ok(())
}
