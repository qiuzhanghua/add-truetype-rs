# add-truetype — 命令行安装 .ttf / .otf / .ttc 字体（当前用户）

`add-truetype` 是一个纯命令行工具，把 `.ttf` / `.otf` / `.ttc` 字体安装到 **当前用户**
（无需管理员权限），支持 macOS / Linux / Windows。

## 功能

- **自动模式**（不带路径参数，缺省为当前目录 `.`）：扫描当前目录（仅顶层）与
  `ttf/`、`otf/` 子目录（递归，目录名大小写不敏感）中的 `.ttf` / `.otf` / `.ttc` 字体文件；
  同时识别当前目录顶层的压缩包（`.zip` / `.tar.gz` / `.tgz` / `.tar.zst` / `.tar`），
  自动解压到临时目录、递归安装其中的字体，安装后清理临时目录。
- **路径模式**：命令后直接给出路径（文件或目录，如 `add-truetype file.otf
  file.ttf file.tar.gz somedir`），只处理给出的路径——文件按类型直接处理，
  目录按自动扫描规则递归处理。
- **压缩包支持**：自动解压并安装包内字体，带路径穿越等安全防护（见下文）。
- 所有模式都支持 `--dry-run` 预览。
- 安装失败时（如字体被其他程序占用）打印错误并**继续安装其余字体**，最后汇总列出所有失败的字体。

## 构建

```bash
cargo build --release        # 产物: target/release/add-truetype
cargo test                   # 运行单元测试
```

> 依赖：macOS / Linux 下全部为纯 Rust 依赖；Windows 下额外使用 `ttf-parser`
> （解析字体内部名称）与 `winreg`（写入注册表）。Windows 上构建即可得到
> Windows 可执行文件。

## 用法

```bash
add-truetype             # 缺省扫描当前目录（等价 add-truetype .）
add-truetype --dry-run   # 预览，不修改任何文件（-n）
add-truetype --verbose   # 输出详细信息（-v）
add-truetype --help      # 帮助（-h）
add-truetype --version   # 版本号（-V）

# 路径模式：只处理给出的路径（缺省为当前目录）
add-truetype file.otf file.ttf file.tar.gz
add-truetype somedir                       # 目录按自动扫描规则递归处理
add-truetype --dry-run pack.zip myfont.ttf
add-truetype -- --dash-name.ttf   # 文件名以 - 开头时，用 -- 分隔
```

路径模式规则：

- `.ttf` / `.otf` / `.ttc` 文件直接安装；压缩包解压后安装其中的字体；**目录**按自动扫描
  规则处理（顶层 + `ttf/`/`otf/` 子目录递归 + 顶层压缩包）。
- 不存在的路径、坏压缩包或无关文件：打印警告并跳过；若最终没有可安装的字体，
  退出码为 1。
- 只处理命令行给出的路径；不传路径时缺省为当前目录（`add-truetype` 等价
  `add-truetype .`），不会去扫描其他位置。

## 目标目录（各平台）

| 平台 | 目录 | 附加步骤 |
| --- | --- | --- |
| macOS | `~/Library/Fonts` | 无需刷新缓存 |
| Linux | `$XDG_DATA_HOME/fonts`（默认 `~/.local/share/fonts`） | 自动执行 `fc-cache -f` |
| Windows | `%LOCALAPPDATA%\Microsoft\Windows\Fonts` | 注册到 `HKCU:\Software\Microsoft\Windows NT\CurrentVersion\Fonts` |

## 压缩包支持

当前目录顶层的 `.zip`、`.tar.gz`、`.tgz`、`.tar.zst`、`.tar` 会被自动识别，
也可以在命令后显式指定压缩包文件（`add-truetype pack.zip`）。

- 解压到系统临时目录（`/tmp/add-truetype-*`），递归找出其中的 `.ttf/.otf/.ttc` 安装，
  安装结束后自动删除临时目录（即使中途出错也会清理）。
- 同一字体出现在多个压缩包（或与散装字体重名）时按文件名去重/覆盖。
- 坏掉的压缩包、无法复制的字体（被占用等）只打印警告、跳过，不影响其他字体；最后汇总列出所有失败的字体。
- `--dry-run` 时不解压，直接列出包内的字体条目。
- 安全防护：拒绝 `../` 路径穿越、绝对路径、符号链接等条目（`zip` 的
  `enclosed_name` + 自实现的 `sanitize_join`），恶意压缩包无法把文件写到
  临时目录之外。
- 注意：只处理当前目录**顶层**的压缩包，不递归解压"压缩包里的压缩包"；
  zip 内条目若使用 bzip2/xz/lzma/zstd 等非常见压缩方式会提示无法解压
  （常见 zip 均为 deflate/存储压缩，不受影响）。

## Windows 说明

- 按用户安装字体需要 Windows 10 1809 或更新版本；更早版本会忽略 HKCU 下的
  字体注册，只能以管理员身份安装到 `C:\Windows\Fonts`。
- 注册表值名通过 `ttf-parser` 直接解析字体文件的 family 名称（优先 Windows
  平台名称），比按文件名注册更可靠；解析失败时退回文件名并给出警告。
- `.ttc`（TrueType Collection）文件包含多个字体，Windows 注册时会为每个 face
  独立创建注册表条目，同名 face 会自动添加序号后缀（如 "Arial"、"Arial 2"）。

## 注意事项

- 只做 **按用户** 安装，不会污染系统字体目录；卸载时删除字体文件及对应注册表项即可。
- 安装后若应用没有立即显示新字体，请重启该应用；个别系统（macOS）可能需要注销重登。
- Linux 下若想对所有用户生效，需以管理员身份复制到 `/usr/share/fonts` 或
  `/usr/local/share/fonts`，再执行 `fc-cache -f`（本工具不处理）。
- WSL 中运行本工具按 Linux 处理；若希望字体在 Windows 侧应用（如 VS Code
  Windows 版）中可见，需将字体复制到 Windows 的字体目录（例如
  `/mnt/c/Windows/Fonts`），不在本工具范围内。

## 项目结构

| 文件 | 说明 |
| --- | --- |
| `src/main.rs` | 主程序：clap 命令行解析、目录/压缩包扫描、安装逻辑、单元测试 |
| `src/fontname.rs` | 读取字体 family 名称（Windows 注册用，仅 Windows 编译） |
| `src/registry.rs` | Windows HKCU 注册表写入（仅 Windows 编译） |
| `Cargo.toml` | 依赖与构建配置 |
