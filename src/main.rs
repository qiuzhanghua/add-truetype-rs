//! add-truetype — 把 .ttf/.otf 字体安装到“当前用户”的命令行工具。
//!
//! 支持 macOS / Linux / Windows（无需管理员权限）：
//! - macOS   : 复制到 ~/Library/Fonts
//! - Linux   : 复制到 $XDG_DATA_HOME/fonts（默认 ~/.local/share/fonts），然后运行 fc-cache -f
//! - Windows : 复制到 %LOCALAPPDATA%\Microsoft\Windows\Fonts，并注册到
//!             HKCU\Software\Microsoft\Windows NT\CurrentVersion\Fonts
//!
//! 字体来源：
//! - 当前目录（仅顶层）与 ttf/ otf/ 子目录（递归）中的 .ttf/.otf 文件；
//! - 当前目录顶层的压缩包：.zip / .tar.gz / .tgz / .tar.zst / .tar，
//!   会自动解压到临时目录、递归安装其中的字体，完成后清理临时目录。

// 字体内部名称解析仅 Windows 注册时需要（macOS/Linux 不编译该模块）
#[cfg(target_os = "windows")]
mod fontname;

#[cfg(target_os = "windows")]
mod registry;

use std::collections::HashSet;
use std::env;
use std::fs;
use std::fs::File;
use std::io::{self, Read};
use std::path::Component;
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

// 命令行接口由 clap 提供：--help/-h、--version/-V、-- 分隔符等均为原生行为。
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "add-truetype",
    version,
    about = "把 .ttf/.otf 字体安装到当前用户的命令行工具（macOS / Linux / Windows）",
    long_about = r#"把 .ttf/.otf 字体安装到当前用户的命令行工具（macOS / Linux / Windows）。

不带文件参数时（自动模式）：
  扫描当前目录（仅顶层）与 ttf/、otf/ 子目录（递归）中的 .ttf/.otf 字体，
  以及当前目录顶层的压缩包（.zip / .tar.gz / .tgz / .tar.zst / .tar），
  自动解压到临时目录、递归安装其中的字体，安装后清理临时目录。

直接指定路径时（如: add-truetype file.otf file.ttf file.tar.gz 或 add-truetype somedir）：
  不做整盘/全目录扫描，只处理给出的路径——.ttf/.otf 直接安装，压缩包解压后
  安装其中的字体，目录则按自动扫描规则递归处理；不存在的路径或无关文件会警告并跳过。
  不带任何路径参数时缺省为当前目录（等价 add-truetype .）。

目标目录：
  macOS   : ~/Library/Fonts
  Linux   : ~/.local/share/fonts    （安装后自动运行 fc-cache -f）
  Windows : %LOCALAPPDATA%\Microsoft\Windows\Fonts
            （并注册到 HKCU，无需管理员权限）"#,
)]
struct Cli {
    /// 只打印将要执行的操作，不实际修改任何文件
    #[arg(short = 'n', long)]
    dry_run: bool,

    /// 输出详细信息
    #[arg(short, long)]
    verbose: bool,

    /// 要安装的字体/压缩包文件或目录（目录按自动扫描规则递归处理）；
    /// 不指定时缺省为当前目录 "."
    #[arg(value_name = "路径", default_value = ".")]
    files: Vec<PathBuf>,
}

// 变体按编译平台决定：各平台只会构造本平台的变体，
// 其余变体仅出现在单元测试与 match 中，属预期行为。
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Platform {
    Macos,
    Linux,
    Windows,
}

impl Platform {
    fn current() -> Option<Platform> {
        #[cfg(target_os = "macos")]
        {
            Some(Platform::Macos)
        }
        #[cfg(target_os = "linux")]
        {
            Some(Platform::Linux)
        }
        #[cfg(target_os = "windows")]
        {
            Some(Platform::Windows)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            None
        }
    }
}

/// 计算“当前用户”的字体目录（纯函数，便于跨平台单元测试）。
fn font_dir_for(
    platform: Platform,
    home: Option<&Path>,
    xdg_data_home: Option<&Path>,
    local_app_data: Option<&Path>,
) -> Option<PathBuf> {
    match platform {
        Platform::Macos => home.map(|h| h.join("Library").join("Fonts")),
        Platform::Linux => match xdg_data_home {
            Some(x) => Some(x.join("fonts")),
            None => home.map(|h| h.join(".local").join("share").join("fonts")),
        },
        Platform::Windows => match local_app_data {
            Some(l) => Some(l.join("Microsoft").join("Windows").join("Fonts")),
            None => home.map(|h| {
                h.join("AppData")
                    .join("Local")
                    .join("Microsoft")
                    .join("Windows")
                    .join("Fonts")
            }),
        },
    }
}

fn user_font_dir() -> Option<PathBuf> {
    let platform = Platform::current()?;
    font_dir_for(
        platform,
        env::var_os("HOME")
            .or_else(|| env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .as_deref(),
        env::var_os("XDG_DATA_HOME").map(PathBuf::from).as_deref(),
        env::var_os("LOCALAPPDATA").map(PathBuf::from).as_deref(),
    )
}

fn is_font_file(p: &Path) -> bool {
    match p.extension().and_then(|e| e.to_str()) {
        Some(e) => {
            e.eq_ignore_ascii_case("ttf")
                || e.eq_ignore_ascii_case("otf")
                || e.eq_ignore_ascii_case("ttc")
        }
        None => false,
    }
}

/// 递归收集目录下所有字体文件。
/// 用 `file_type` 判断（不跟随符号链接），避免解压目录中的恶意链接逃逸。
fn walk_dir(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        let path = entry.path();
        if ft.is_dir() {
            walk_dir(&path, out);
        } else if ft.is_file() && is_font_file(&path) {
            out.push(path);
        }
    }
}

/// 收集字体：当前目录顶层 + ttf/ otf/ 子目录（递归），并去重。
fn collect_fonts(cwd: &Path) -> Vec<PathBuf> {
    let mut fonts = Vec::new();

    // 当前目录顶层
    if let Ok(rd) = fs::read_dir(cwd) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_file() && is_font_file(&path) {
                fonts.push(path);
            }
        }
    }

    // ttf / otf 子目录（递归）。macOS 默认文件系统大小写不敏感，
    // ttf 与 TTF 是同一目录，因此每类只取“第一个实际存在”的候选，
    // 同时兼容 Linux 上只有大写目录（TTF/OTF）的情况。
    for base in ["ttf", "otf"] {
        let lower = cwd.join(base);
        let upper = cwd.join(base.to_ascii_uppercase());
        let dir = if lower.is_dir() {
            lower
        } else if upper.is_dir() {
            upper
        } else {
            continue;
        };
        walk_dir(&dir, &mut fonts);
    }

    // 去重并保持顺序
    let mut seen = HashSet::new();
    fonts.into_iter().filter(|f| seen.insert(f.clone())).collect()
}

// ---------------------------------------------------------------------------
// 压缩包支持：.zip / .tar.gz / .tgz / .tar.zst / .tar
// ---------------------------------------------------------------------------

/// 返回压缩包类型（"zip" / "tar.gz" / "tar.zst" / "tar"），不是压缩包则返回 None。
fn archive_kind(p: &Path) -> Option<&'static str> {
    let name = p.file_name()?.to_str()?.to_ascii_lowercase();
    if name.ends_with(".zip") {
        Some("zip")
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some("tar.gz")
    } else if name.ends_with(".tar.zst") {
        Some("tar.zst")
    } else if name.ends_with(".tar") {
        Some("tar")
    } else {
        None
    }
}

fn is_archive_file(p: &Path) -> bool {
    archive_kind(p).is_some()
}

/// 收集当前目录顶层的压缩包（排序保证输出顺序稳定）。
fn collect_archives(cwd: &Path) -> Vec<PathBuf> {
    let mut archives = Vec::new();
    if let Ok(rd) = fs::read_dir(cwd) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_file() && is_archive_file(&path) {
                archives.push(path);
            }
        }
    }
    archives.sort();
    archives
}

/// 把压缩包内的相对路径安全地拼到目标目录下；
/// 含 `..`、绝对路径或盘符前缀的条目返回 None（拒绝该条目，防止路径穿越）。
fn sanitize_join(base: &Path, rel: &Path) -> Option<PathBuf> {
    let mut out = base.to_path_buf();
    for comp in rel.components() {
        match comp {
            Component::Normal(s) => out.push(s),
            Component::CurDir => {}
            _ => return None, // ParentDir / RootDir / Prefix
        }
    }
    Some(out)
}

/// 打开 tar 系压缩包的读取器（按扩展名选择解压方式）。
fn open_archive_reader(archive: &Path) -> io::Result<Box<dyn Read>> {
    let file = File::open(archive)?;
    match archive_kind(archive) {
        Some("tar.gz") => Ok(Box::new(flate2::read::GzDecoder::new(file))),
        Some("tar.zst") => {
            let decoder = zstd::stream::read::Decoder::new(file)?;
            Ok(Box::new(decoder))
        }
        Some("tar") => Ok(Box::new(file)),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("不支持的压缩包格式: {}", archive.display()),
        )),
    }
}

fn extract_zip(archive: &Path, dest: &Path) -> io::Result<()> {
    let file = File::open(archive)?;
    let mut z = zip::ZipArchive::new(file).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    for i in 0..z.len() {
        let mut entry = z
            .by_index(i)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        if entry.is_dir() {
            continue;
        }
        let Some(rel) = entry.enclosed_name() else {
            eprintln!("警告: 跳过压缩包内不安全的路径: {}", entry.name());
            continue;
        };
        let out = dest.join(rel);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut writer = File::create(&out)?;
        io::copy(&mut entry, &mut writer)?;
    }
    Ok(())
}

fn extract_tar<R: Read>(reader: R, dest: &Path) -> io::Result<()> {
    let mut archive = tar::Archive::new(reader);
    let entries = archive
        .entries()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        // 跳过目录、符号链接、硬链接、设备等，只解压普通文件
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let Ok(rel) = entry.path() else { continue };
        let Some(out) = sanitize_join(dest, &rel) else {
            eprintln!("警告: 跳过解压目标之外的路径: {}", rel.display());
            continue;
        };
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        entry.unpack(&out)?;
    }
    Ok(())
}

/// 解压压缩包到临时目录并找出其中的字体；
/// 临时目录随返回值保存，由 Drop 在作用域结束时自动清理。
struct Extracted {
    temp_dir: PathBuf,
    fonts: Vec<PathBuf>,
}

impl Drop for Extracted {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp_dir);
    }
}

fn extract_archive(archive: &Path) -> io::Result<Extracted> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let temp_dir = std::env::temp_dir().join(format!("add-truetype-{}-{}", std::process::id(), nanos));
    fs::create_dir_all(&temp_dir)?;

    match archive_kind(archive) {
        Some("zip") => extract_zip(archive, &temp_dir)?,
        Some("tar.gz") | Some("tar.zst") | Some("tar") => {
            extract_tar(open_archive_reader(archive)?, &temp_dir)?
        }
        _ => {
            let _ = fs::remove_dir_all(&temp_dir);
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("不支持的压缩包格式: {}", archive.display()),
            ));
        }
    }

    let mut fonts = Vec::new();
    walk_dir(&temp_dir, &mut fonts);
    Ok(Extracted { temp_dir, fonts })
}

/// dry-run：不解压，直接读取压缩包并列出其中的字体条目（含相对路径）。
fn list_archive_fonts(archive: &Path) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    match archive_kind(archive) {
        Some("zip") => {
            let file = File::open(archive)?;
            let mut z =
                zip::ZipArchive::new(file).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            for i in 0..z.len() {
                let entry = z
                    .by_index(i)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                // 与解压逻辑一致：跳过目录与不安全路径（如 ../ 穿越）
                if !entry.is_dir()
                    && entry.enclosed_name().is_some()
                    && is_font_file(Path::new(entry.name()))
                {
                    names.push(entry.name().to_string());
                }
            }
        }
        Some(_) => {
            let mut archive = tar::Archive::new(open_archive_reader(archive)?);
            let entries = archive
                .entries()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            for entry in entries {
                let entry = entry.map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                if !entry.header().entry_type().is_file() {
                    continue;
                }
                if let Ok(p) = entry.path() {
                    // 与解压逻辑一致：跳过不安全路径（如 ../ 穿越）
                    if is_font_file(&p) && sanitize_join(Path::new("."), &p).is_some() {
                        names.push(p.to_string_lossy().into_owned());
                    }
                }
            }
        }
        None => {}
    }
    Ok(names)
}

#[cfg(target_os = "windows")]
fn register_on_windows(src: &Path, dest: &Path, dry_run: bool, verbose: bool) -> std::io::Result<()> {
    let file_name = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let is_otf = src
        .extension()
        .map(|e| e.eq_ignore_ascii_case("otf"))
        .unwrap_or(false);
    let is_ttc = src
        .extension()
        .map(|e| e.eq_ignore_ascii_case("ttc"))
        .unwrap_or(false);

    let all_families = fontname::all_family_names(src);

    if is_ttc && all_families.len() > 1 {
        let mut name_counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        for (face_index, family) in all_families {
            let count = name_counts.entry(family.clone()).or_insert(0);
            *count += 1;
            let unique_name = if *count > 1 {
                format!("{} {}", family, *count)
            } else {
                family
            };

            if let Err(e) = registry::register_font(dest, &unique_name, is_otf, dry_run, verbose) {
                eprintln!("错误: 无法注册 '{}' (face {}): {}", unique_name, face_index, e);
            }
        }
        Ok(())
    } else {
        let family = fontname::family_name(src).unwrap_or_else(|| {
            let stem = src
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            eprintln!("警告: 无法读取 '{}' 的字体名称（可能不是有效字体文件），改用文件名 '{}' 注册。", file_name, stem);
            stem
        });
        registry::register_font(dest, &family, is_otf, dry_run, verbose)
    }
}

/// 复制字体到目标目录；Windows 下额外注册到 HKCU。
/// `total` 为本次安装涉及的字体总数（散装 + 压缩包内），仅用于汇总显示。
struct InstallResult {
    installed: usize,
    overwritten: usize,
    failed: Vec<(PathBuf, String)>,
}

impl InstallResult {
    fn new() -> Self {
        Self {
            installed: 0,
            overwritten: 0,
            failed: Vec::new(),
        }
    }
}

fn install(
    fonts: &[PathBuf],
    dest: &Path,
    dry_run: bool,
    verbose: bool,
    total: usize,
) -> InstallResult {
    let _ = fs::create_dir_all(dest);

    let mut result = InstallResult::new();

    for src in fonts {
        let file_name = src
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let dest_path = dest.join(&file_name);
        let exists = dest_path.exists();

        if dry_run {
            if exists {
                println!("[dry-run] 覆盖: {}", dest_path.display());
            } else {
                println!("[dry-run] 安装: {}", dest_path.display());
            }
            continue;
        }

        if let Err(e) = fs::copy(src, &dest_path) {
            eprintln!("错误: 无法复制 '{}': {}", file_name, e);
            result.failed.push((src.clone(), e.to_string()));
            continue;
        }

        if exists {
            result.overwritten += 1;
            println!("覆盖: {}", file_name);
        } else {
            result.installed += 1;
            println!("安装: {}", file_name);
        }

        if verbose {
            println!("[info] 目标: {}", dest_path.display());
        }

        #[cfg(target_os = "windows")]
        if let Err(e) = register_on_windows(src, &dest_path, dry_run, verbose) {
            eprintln!("错误: 无法注册 '{}': {}", file_name, e);
            result.failed.push((src.clone(), e.to_string()));
        }
    }

    println!(
        "完成: 新增 {} 个，覆盖 {} 个，共 {} 个。",
        result.installed, result.overwritten, total
    );
    result
}

#[cfg(target_os = "linux")]
fn refresh_font_cache(dry_run: bool) {
    if dry_run {
        println!("[dry-run] 将执行: fc-cache -f");
        return;
    }
    match Command::new("fc-cache").arg("-f").status() {
        Ok(status) if status.success() => println!("字体缓存已刷新 (fc-cache -f)。"),
        Ok(_) => eprintln!("警告: fc-cache 执行失败。"),
        Err(_) => eprintln!(
            "警告: 未找到 fc-cache，请安装 fontconfig（如: sudo apt install fontconfig），或稍后手动运行: fc-cache -f"
        ),
    }
}

// ---------------------------------------------------------------------------
// 命令行参数与两种收集模式
// ---------------------------------------------------------------------------

/// 收集结果：待安装字体 + 解压出的临时目录（保持存活到安装完成，Drop 自动清理）
/// + 来自压缩包的字体计数。
struct Collected {
    fonts: Vec<PathBuf>,
    extracted: Vec<Extracted>,
    archive_font_count: usize,
}

/// 按给定路径列表收集字体（缺省为当前目录 "."，由 clap 的 default_value 提供）。
///
/// - 目录：按自动扫描规则处理（顶层 + ttf/ otf/ 子目录递归 + 顶层压缩包）；
/// - 文件：.ttf/.otf 直接收集，压缩包解压后收集其中的字体；
/// - 不存在的路径或无关文件：警告并跳过。
fn collect_paths(paths: &[PathBuf], dest: &Path, dry_run: bool) -> Collected {
    let mut fonts = Vec::new();
    let mut extracted = Vec::new();
    let mut archive_font_count = 0usize;

    for p in paths {
        if !p.exists() {
            eprintln!("警告: 跳过（路径不存在）: {}", p.display());
            continue;
        }
        if p.is_dir() {
            collect_dir(p, dest, dry_run, &mut fonts, &mut extracted, &mut archive_font_count);
            continue;
        }
        if is_font_file(p) {
            fonts.push(p.clone());
        } else if is_archive_file(p) {
            collect_archive(p, dest, dry_run, &mut fonts, &mut extracted, &mut archive_font_count);
        } else {
            eprintln!(
                "警告: 跳过（既不是 .ttf/.otf 字体，也不是支持的压缩包）: {}",
                p.display()
            );
        }
    }

    // 去重并保持顺序
    let mut seen = HashSet::new();
    fonts = fonts.into_iter().filter(|f| seen.insert(f.clone())).collect();

    Collected {
        fonts,
        extracted,
        archive_font_count,
    }
}

/// 按自动扫描规则处理一个目录：顶层字体 + ttf/ otf/ 子目录（递归）+ 顶层压缩包。
fn collect_dir(
    dir: &Path,
    dest: &Path,
    dry_run: bool,
    fonts: &mut Vec<PathBuf>,
    extracted: &mut Vec<Extracted>,
    archive_font_count: &mut usize,
) {
    fonts.extend(collect_fonts(dir));
    for arch in collect_archives(dir) {
        collect_archive(&arch, dest, dry_run, fonts, extracted, archive_font_count);
    }
}

/// 处理单个压缩包：dry-run 时只列出内容，否则解压并收集其中的字体。
fn collect_archive(
    arch: &Path,
    dest: &Path,
    dry_run: bool,
    fonts: &mut Vec<PathBuf>,
    extracted: &mut Vec<Extracted>,
    archive_font_count: &mut usize,
) {
    if dry_run {
        match list_archive_fonts(arch) {
            Ok(names) if names.is_empty() => {
                println!("[dry-run] 压缩包 {} 内没有字体文件。", arch.display());
            }
            Ok(names) => {
                println!(
                    "[dry-run] 压缩包 {} 内找到 {} 个字体:",
                    arch.display(),
                    names.len()
                );
                *archive_font_count += names.len();
                for name in &names {
                    let base = Path::new(name).file_name().unwrap_or_default();
                    println!(
                        "[dry-run]   {} -> {}",
                        name,
                        dest.join(base).display()
                    );
                }
            }
            Err(e) => eprintln!("警告: 读取 {} 失败: {}", arch.display(), e),
        }
    } else {
        match extract_archive(arch) {
            Ok(ex) => {
                if ex.fonts.is_empty() {
                    println!("压缩包 {} 内没有字体文件。", arch.display());
                } else {
                    println!(
                        "压缩包 {} 内找到 {} 个字体。",
                        arch.display(),
                        ex.fonts.len()
                    );
                    *archive_font_count += ex.fonts.len();
                }
                fonts.extend(ex.fonts.iter().cloned());
                extracted.push(ex);
            }
            Err(e) => eprintln!("警告: 解压 {} 失败: {}", arch.display(), e),
        }
    }
}

fn main() {
    // clap 负责参数解析：--help/-h、--version/-V、-- 分隔符、未知参数报错等
    let cli = Cli::parse();
    let dry_run = cli.dry_run;
    let verbose = cli.verbose;

    let Some(platform) = Platform::current() else {
        eprintln!("错误: 不支持的操作系统。");
        std::process::exit(1);
    };
    let Some(dest) = user_font_dir() else {
        eprintln!("错误: 无法确定用户字体目录（未找到 HOME/USERPROFILE 环境变量）。");
        std::process::exit(1);
    };

    // cli.files 缺省为 ["."]，未传参时即扫描当前目录（clap default_value）
    let collected = collect_paths(&cli.files, &dest, dry_run);
    let fonts = collected.fonts;
    // 解压出的临时目录保持存活到 main 结束（作用域结束时 Drop 自动清理）
    let _extracted = collected.extracted;
    let archive_font_count = collected.archive_font_count;

    if fonts.is_empty() && archive_font_count == 0 {
        eprintln!("错误: 未找到任何可安装的 .ttf/.otf 字体或有效压缩包（已扫描指定路径，缺省为当前目录）。");
        std::process::exit(1);
    }
    let total = if dry_run {
        fonts.len() + archive_font_count
    } else {
        fonts.len()
    };
    if archive_font_count > 0 {
        println!(
            "共找到 {} 个字体文件（其中 {} 个来自压缩包）。",
            total, archive_font_count
        );
    } else {
        println!("共找到 {} 个字体文件。", total);
    }

    if verbose {
        println!("[info] 目标目录: {}", dest.display());
        if dry_run {
            println!("[info] 模式: dry-run（不会修改任何文件）");
        }
    }

    let result = install(&fonts, &dest, dry_run, verbose, total);
    if !result.failed.is_empty() {
        eprintln!("\n以下字体安装失败（可能被其他程序占用）：");
        for (path, err) in &result.failed {
            let name = path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            eprintln!("  - {}: {}", name, err);
        }
    }

    #[cfg(target_os = "linux")]
    refresh_font_cache(dry_run);

    match platform {
        Platform::Macos => {
            println!("macOS: 字体已安装到 ~/Library/Fonts，重启相关应用（或注销重登）后即可使用。");
        }
        Platform::Linux => {
            println!(
                "Linux: 字体已安装到 {}，重启相关应用后即可使用。",
                dest.display()
            );
        }
        Platform::Windows => {
            println!("Windows: 字体已安装并注册到当前用户，重启相关应用后即可使用。");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_font_dir() {
        let home = Path::new("/Users/test");
        assert_eq!(
            font_dir_for(Platform::Macos, Some(home), None, None),
            Some(PathBuf::from("/Users/test/Library/Fonts"))
        );
    }

    #[test]
    fn linux_font_dir_uses_xdg() {
        let home = Path::new("/home/test");
        let xdg = Path::new("/home/test/.local/share");
        assert_eq!(
            font_dir_for(Platform::Linux, Some(home), Some(xdg), None),
            Some(PathBuf::from("/home/test/.local/share/fonts"))
        );
    }

    #[test]
    fn linux_font_dir_default() {
        let home = Path::new("/home/test");
        assert_eq!(
            font_dir_for(Platform::Linux, Some(home), None, None),
            Some(PathBuf::from("/home/test/.local/share/fonts"))
        );
    }

    #[test]
    fn windows_font_dir_uses_localappdata() {
        let home = Path::new(r"C:\Users\test");
        let la = Path::new(r"C:\Users\test\AppData\Local");
        // 期望值同样用 join 构造，保证在非 Windows 主机上分隔符一致
        let expected = la
            .join("Microsoft")
            .join("Windows")
            .join("Fonts");
        assert_eq!(
            font_dir_for(Platform::Windows, Some(home), None, Some(la)),
            Some(expected)
        );
    }

    #[test]
    fn windows_font_dir_fallback_to_home() {
        let home = Path::new(r"C:\Users\test");
        let expected = home
            .join("AppData")
            .join("Local")
            .join("Microsoft")
            .join("Windows")
            .join("Fonts");
        assert_eq!(
            font_dir_for(Platform::Windows, Some(home), None, None),
            Some(expected)
        );
    }

    #[test]
    fn collect_fonts_scans_expected_locations_only() {
        let base = std::env::temp_dir().join(format!("add-truetype-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("ttf").join("nested")).unwrap();
        fs::create_dir_all(base.join("otf")).unwrap();
        fs::create_dir_all(base.join("sub")).unwrap();
        for f in [
            "Top.TTF",
            "top.otf",
            "ttf/One.ttf",
            "ttf/nested/Two.otf",
            "otf/Three.otf",
            "sub/Skip.ttf",
            "note.txt",
        ] {
            fs::write(base.join(f), "").unwrap();
        }

        let fonts = collect_fonts(&base);
        let mut got: Vec<String> = fonts
            .iter()
            .map(|p| {
                p.strip_prefix(&base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
                    .replace('\\', "/")
            })
            .collect();
        got.sort();

        let mut expected = vec![
            "Top.TTF".to_string(),
            "top.otf".to_string(),
            "ttf/One.ttf".to_string(),
            "ttf/nested/Two.otf".to_string(),
            "otf/Three.otf".to_string(),
        ];
        expected.sort();
        assert_eq!(got, expected);

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        assert!(is_font_file(Path::new("A.TTF")));
        assert!(is_font_file(Path::new("a.ttf")));
        assert!(is_font_file(Path::new("B.Otf")));
        assert!(is_font_file(Path::new("b.otf")));
        assert!(is_font_file(Path::new("C.TTC")));
        assert!(is_font_file(Path::new("c.ttc")));
        assert!(!is_font_file(Path::new("note.txt")));
        assert!(!is_font_file(Path::new("noext")));
    }

    // ---- 压缩包 ----

    fn test_base(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "add-truetype-test-{}-{}",
            name,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn archive_detection() {
        assert!(is_archive_file(Path::new("a.zip")));
        assert!(is_archive_file(Path::new("A.TAR.GZ")));
        assert!(is_archive_file(Path::new("b.tgz")));
        assert!(is_archive_file(Path::new("c.tar.zst")));
        assert!(is_archive_file(Path::new("d.tar")));
        assert!(!is_archive_file(Path::new("e.ttf")));
        assert!(!is_archive_file(Path::new("f.zip.bak")));
        assert!(!is_archive_file(Path::new("note.txt")));
    }

    #[test]
    fn zip_archive_extraction_finds_fonts_and_skips_traversal() {
        use std::io::Write;

        let base = test_base("zip");
        let zip_path = base.join("fonts.zip");
        let file = fs::File::create(&zip_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("FontA.ttf", opts).unwrap();
        writer.write_all(b"dummy").unwrap();
        writer.start_file("nested/FontB.otf", opts).unwrap();
        writer.write_all(b"dummy").unwrap();
        writer.start_file("readme.txt", opts).unwrap();
        writer.write_all(b"x").unwrap();
        writer.start_file("../evil.ttf", opts).unwrap(); // 路径穿越
        writer.write_all(b"dummy").unwrap();
        writer.finish().unwrap();

        let extracted = extract_archive(&zip_path).unwrap();
        let mut names: Vec<String> = extracted
            .fonts
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["FontA.ttf", "FontB.otf"]);

        // dry-run 列表同样只报告合法字体条目
        let listed = list_archive_fonts(&zip_path).unwrap();
        assert!(listed.contains(&"FontA.ttf".to_string()));
        assert!(listed.contains(&"nested/FontB.otf".to_string()));
        assert!(!listed.iter().any(|n| n.contains("evil")));
        assert!(!listed.iter().any(|n| n.contains("readme")));

        // 解压后的临时目录被自动清理
        let temp = extracted.temp_dir.clone();
        drop(extracted);
        assert!(!temp.exists());

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn targz_archive_extraction_finds_fonts() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use tar::Header;

        let base = test_base("targz");
        let tgz_path = base.join("fonts.tar.gz");
        let file = fs::File::create(&tgz_path).unwrap();
        let enc = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(enc);

        let mut hdr = Header::new_gnu();
        hdr.set_size(5);
        hdr.set_mode(0o644);
        hdr.set_cksum();
        builder
            .append_data(&mut hdr, "FontC.ttf", &b"dummy"[..])
            .unwrap();

        let mut hdr = Header::new_gnu();
        hdr.set_size(5);
        hdr.set_mode(0o644);
        hdr.set_cksum();
        builder
            .append_data(&mut hdr, "nested/FontD.otf", &b"dummy"[..])
            .unwrap();

        builder.finish().unwrap(); // 写入 tar 尾部填充
        let enc = builder.into_inner().unwrap();
        enc.finish().unwrap(); // 冲刷 gzip 帧

        let extracted = extract_archive(&tgz_path).unwrap();
        let mut names: Vec<String> = extracted
            .fonts
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["FontC.ttf", "FontD.otf"]);

        let temp = extracted.temp_dir.clone();
        drop(extracted);
        assert!(!temp.exists());

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn sanitize_join_rejects_traversal() {
        let base = Path::new("/tmp/base");
        assert_eq!(
            sanitize_join(base, Path::new("a/b.ttf")),
            Some(PathBuf::from("/tmp/base/a/b.ttf"))
        );
        assert_eq!(
            sanitize_join(base, Path::new("./c.ttf")),
            Some(PathBuf::from("/tmp/base/c.ttf"))
        );
        assert_eq!(sanitize_join(base, Path::new("../evil.ttf")), None);
        assert_eq!(sanitize_join(base, Path::new("/abs.ttf")), None);
        #[cfg(target_os = "windows")]
        {
            // Windows 上盘符前缀属于 Prefix 组件，应被拒绝
            assert_eq!(sanitize_join(base, Path::new(r"C:\evil.ttf")), None);
        }
        #[cfg(not(target_os = "windows"))]
        {
            // 非 Windows 上反斜杠不是路径分隔符，只是普通文件名
            assert_eq!(
                sanitize_join(base, Path::new(r"C:\evil.ttf")),
                Some(PathBuf::from(r"/tmp/base/C:\evil.ttf"))
            );
        }
    }

    #[test]
    fn tarzst_archive_extraction_finds_fonts() {
        use tar::Header;

        let base = test_base("tarzst");
        let tzst_path = base.join("fonts.tar.zst");
        let file = fs::File::create(&tzst_path).unwrap();
        let enc = zstd::stream::write::Encoder::new(file, 3).unwrap();
        let mut builder = tar::Builder::new(enc);

        let mut hdr = Header::new_gnu();
        hdr.set_size(5);
        hdr.set_mode(0o644);
        hdr.set_cksum();
        builder
            .append_data(&mut hdr, "FontE.ttf", &b"dummy"[..])
            .unwrap();

        builder.finish().unwrap(); // 写入 tar 尾部填充
        let enc = builder.into_inner().unwrap();
        enc.finish().unwrap(); // 冲刷 zstd 帧

        let extracted = extract_archive(&tzst_path).unwrap();
        let names: Vec<String> = extracted
            .fonts
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["FontE.ttf"]);

        let temp = extracted.temp_dir.clone();
        drop(extracted);
        assert!(!temp.exists());

        fs::remove_dir_all(&base).unwrap();
    }

    // ---- 命令行参数（clap）与显式文件模式 ----

    #[test]
    fn cli_parses_options_and_files() {
        let cli =
            Cli::try_parse_from(["add-truetype", "-n", "--verbose", "a.ttf", "b.tar.gz"]).unwrap();
        assert!(cli.dry_run);
        assert!(cli.verbose);
        assert_eq!(
            cli.files,
            vec![PathBuf::from("a.ttf"), PathBuf::from("b.tar.gz")]
        );
    }

    #[test]
    fn cli_dashdash_allows_dash_filenames() {
        let cli = Cli::try_parse_from(["add-truetype", "--", "--weird.ttf", "-n"]).unwrap();
        assert!(!cli.dry_run);
        assert_eq!(cli.files, vec![PathBuf::from("--weird.ttf"), PathBuf::from("-n")]);
    }

    #[test]
    fn cli_default_files_is_current_dir() {
        // 未传路径时缺省为当前目录 "."
        let cli = Cli::try_parse_from(["add-truetype"]).unwrap();
        assert_eq!(cli.files, vec![PathBuf::from(".")]);
        let cli = Cli::try_parse_from(["add-truetype", "-n"]).unwrap();
        assert_eq!(cli.files, vec![PathBuf::from(".")]);
    }

    #[test]
    fn cli_help_and_version_flags() {
        use clap::error::ErrorKind;

        let err = Cli::try_parse_from(["add-truetype", "--help"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DisplayHelp);
        let err = Cli::try_parse_from(["add-truetype", "-h"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DisplayHelp);

        let err = Cli::try_parse_from(["add-truetype", "--version"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DisplayVersion);
        let err = Cli::try_parse_from(["add-truetype", "-V"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DisplayVersion);
    }

    #[test]
    fn cli_rejects_unknown_option() {
        use clap::error::ErrorKind;

        let err = Cli::try_parse_from(["add-truetype", "--bogus"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::UnknownArgument);
    }

    #[test]
    fn paths_mode_only_processes_given_paths() {
        use std::io::Write;

        let base = test_base("paths");
        fs::write(base.join("Given.ttf"), "dummy").unwrap();
        fs::write(base.join("note.txt"), "x").unwrap();
        fs::create_dir_all(base.join("adir")).unwrap();
        fs::write(base.join("adir/Inside.ttf"), "dummy").unwrap();

        // 压缩包：内含一个字体
        let zip_path = base.join("fonts.zip");
        let file = fs::File::create(&zip_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("Z1.ttf", opts).unwrap();
        writer.write_all(b"dummy").unwrap();
        writer.finish().unwrap();

        let dest = base.join("out");
        let paths = vec![
            base.join("Given.ttf"),
            zip_path.clone(),
            base.join("note.txt"),     // 无关文件 → 跳过
            base.join("missing.ttf"),  // 不存在 → 跳过
            base.join("adir"),         // 目录 → 按自动扫描规则处理
        ];
        let collected = collect_paths(&paths, &dest, false);

        let mut names: Vec<String> = collected
            .fonts
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["Given.ttf", "Inside.ttf", "Z1.ttf"]);
        assert_eq!(collected.archive_font_count, 1);

        // 解压出的临时目录被自动清理
        for ex in collected.extracted {
            let temp = ex.temp_dir.clone();
            drop(ex);
            assert!(!temp.exists());
        }

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn collect_paths_scans_given_directory() {
        let base = test_base("paths-dir");
        fs::write(base.join("D1.ttf"), "dummy").unwrap();
        fs::create_dir_all(base.join("ttf")).unwrap();
        fs::write(base.join("ttf/D2.otf"), "dummy").unwrap();
        fs::write(base.join("note.txt"), "x").unwrap();

        let dest = base.join("out");
        let collected = collect_paths(&[base.clone()], &dest, false);

        let mut names: Vec<String> = collected
            .fonts
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["D1.ttf", "D2.otf"]);

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn paths_dry_run_lists_archive_fonts() {
        use std::io::Write;

        let base = test_base("paths-dry");
        let zip_path = base.join("fonts.zip");
        let file = fs::File::create(&zip_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("Z1.ttf", opts).unwrap();
        writer.write_all(b"dummy").unwrap();
        writer.finish().unwrap();

        let dest = base.join("out");
        let collected = collect_paths(&[zip_path.clone()], &dest, true);
        // dry-run 不解压：fonts 为空、计数来自列表
        assert!(collected.fonts.is_empty());
        assert_eq!(collected.archive_font_count, 1);
        assert!(collected.extracted.is_empty());

        fs::remove_dir_all(&base).unwrap();
    }
}
