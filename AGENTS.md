# AGENTS.md

## 项目概述

`add-truetype` 是一个纯命令行工具，将 `.ttf` / `.otf` / `.ttc` 字体安装到当前用户目录（无需管理员权限），支持 macOS / Linux / Windows。

## 源码结构

| 文件 | 说明 |
| --- | --- |
| `src/main.rs` | 主程序：clap CLI 解析、目录/压缩包扫描、字体安装逻辑、单元测试 |
| `src/fontname.rs` | 读取字体 family 名称（Windows 注册用，**仅 Windows 编译**） |
| `src/registry.rs` | Windows HKCU 注册表写入（**仅 Windows 编译**） |
| `Cargo.toml` | 依赖与构建配置 |

## 构建与测试

```bash
cargo build              # 开发构建
cargo build --release    # 发布构建
cargo test               # 运行单元测试
```

## 关键实现说明

### 平台检测
- `Platform::current()` 通过 `#[cfg(target_os = "...")]` 条件编译返回当前平台
- 三种变体：`Platform::Macos` / `Platform::Linux` / `Platform::Windows`

### 字体收集流程
1. **自动模式**（路径为空或为 `.`）：扫描当前目录顶层 + `ttf/`/`otf/` 子目录（递归，目录名大小写不敏感）+ 顶层压缩包
2. **路径模式**（显式传入路径）：按类型处理——`.ttf/.otf/.ttc` 直接收集，压缩包解压后收集，目录则按自动扫描规则处理

### 压缩包支持
- 支持格式：`.zip` / `.7z` / `.tar.gz` / `.tgz` / `.tar.zst` / `.tar`
- 解压到系统临时目录（`add-truetype-<pid>-<nanos>`），安装结束后自动清理（即使出错也会 Drop 清理）
- `tar` 系使用 `flate2::GzDecoder`（tar.gz/tgz）或 `zstd` 解压
- `zip` 使用 `zip::ZipArchive`
- `.7z` 使用 `sevenz-rust`（纯 Rust LZMA 解码，无 C 依赖）：`extract_7z()` 解压、
  列表分支直接从 `SevenZReader.archive().files` 读条目名（dry-run 不解压数据）
- 路径穿越防护：`sanitize_join` 拒绝 `..`、绝对路径、Prefix（Windows 盘符）等

### Windows 注册
- 字体复制到 `%LOCALAPPDATA%\Microsoft\Windows\Fonts`
- 注册表写入 `HKCU\Software\Microsoft\Windows NT\CurrentVersion\Fonts`
- 注册表值名通过 `ttf-parser` 解析字体内部 family 名称，失败则退回文件名
- `.ttc` 文件中每个 face 独立注册，同名 face 自动加序号后缀区分

### 安装错误处理
- 复制失败（如字体被其他程序占用）打印错误并**继续**安装其余字体
- Windows 注册失败同理
- 最后汇总列出所有失败的字体（不影响已成功安装的）

## 代码风格

- 无注释要求（除非解释逻辑）
- 模块级条件编译：`#[cfg(target_os = "windows")]`
- 使用 `ttf-parser`（`fontname` 模块）和 `winreg`（`registry` 模块）处理 Windows 特化逻辑
- 错误处理：返回 `io::Result` 或自定义 `InstallResult`，避免 `unwrap()` 除非测试中

## 常见编辑场景

### 添加新压缩包格式
1. 在 `archive_kind()` 添加扩展名判断
2. 在 `open_archive_reader()` 添加对应读取器（仅流式格式如 tar 系需要）
3. 在 `extract_archive()` 的 match 中添加解压分支（非流式格式如 7z 用独立的 `extract_7z()` 等函数）
4. 在 `list_archive_fonts()` 添加列表读取分支
5. 添加测试：能用写入器现场生成的格式（zip/tar 系）程序化构造；只读格式（如 7z，
   `sevenz-rust` 无写入能力）在 `tests/data/` 放内置样例，用 `include_bytes!` 读取

### 添加新平台
1. 在 `Platform` 枚举添加变体
2. 在 `Platform::current()` 添加 `#[cfg]` 分支
3. 在 `font_dir_for()` 添加目录计算逻辑
4. 在 `main()` 的 platform match 添加平台消息

### 修改安装逻辑
- `install()` 函数是核心入口，返回 `InstallResult`
- Windows 注册在 `register_on_windows()` 中
- Linux 刷新缓存在 `refresh_font_cache()` 中
