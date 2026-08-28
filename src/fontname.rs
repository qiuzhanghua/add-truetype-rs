//! 读取 TTF/OTF/TTC 字体的内部名称（family name）。
//!
//! Windows 按用户安装字体时，注册表值名必须与字体内部名称一致
//! （例如 "MyFont (TrueType)"），应用才能正确识别该字体。

use std::path::Path;

fn family_name_from_face(face: &ttf_parser::Face) -> Option<String> {
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

/// 返回字体的 family name（face_index = 0）。
///
/// 优先返回 Windows 平台（platform 3）下的 family 名称；如果没有，
/// 退回任意平台的第一个 family 名称。解析失败时返回 `None`，
/// 由调用方决定是否改用文件名。
pub fn family_name(path: &Path) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    let face = ttf_parser::Face::parse(&data, 0).ok()?;
    family_name_from_face(&face)
}

/// 返回指定 face_index 的 family name。
pub fn family_name_for_face(path: &Path, face_index: u32) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    let face = ttf_parser::Face::parse(&data, face_index).ok()?;
    family_name_from_face(&face)
}

/// 返回字体文件中所有 face 的 family name（用于 TTC）。
pub fn all_family_names(path: &Path) -> Vec<(u32, String)> {
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };

    let mut names = Vec::new();
    let mut face_index: u32 = 0;
    loop {
        match ttf_parser::Face::parse(&data, face_index) {
            Ok(face) => {
                if let Some(name) = family_name_from_face(&face) {
                    names.push((face_index, name));
                }
            }
            Err(_) => break,
        }
        face_index += 1;
    }
    names
}
