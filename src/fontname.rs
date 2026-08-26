//! 读取 TTF/OTF 字体的内部名称（family name）。
//!
//! Windows 按用户安装字体时，注册表值名必须与字体内部名称一致
//! （例如 "MyFont (TrueType)"），应用才能正确识别该字体。

use std::path::Path;

/// 返回字体的 family name。
///
/// 优先返回 Windows 平台（platform 3）下的 family 名称；如果没有，
/// 退回任意平台的第一个 family 名称。解析失败时返回 `None`，
/// 由调用方决定是否改用文件名。
pub fn family_name(path: &Path) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    let face = ttf_parser::Face::parse(&data, 0).ok()?;

    let mut first: Option<String> = None;
    let mut windows_name: Option<String> = None;

    for name in face.names() {
        if name.name_id != ttf_parser::name_id::FAMILY {
            continue;
        }
        if first.is_none() {
            first = name.to_string();
        }
        if windows_name.is_none() && name.platform_id == ttf_parser::PlatformId::Windows {
            windows_name = name.to_string();
        }
    }

    windows_name.or(first)
}
