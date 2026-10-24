# 发布与打包

> 状态：设计稿（2026-10-24，M20）。已实现：`mw-crystal`、`mw-crystal-io` 的 crates.io 元数据和文档（`cargo publish --dry-run` 通过，第 6 节），依赖许可的检查（`deny.toml`，第 7.1 节）。未实现：许可清单的生成（等你同意安装 cargo-about，第 7.2 节）、安装包、Release 工作流、应用侧的打开文件和更新检查。要你决定的事项在第 10 节，实施顺序在第 11 节。
> 依据：路线图第 7 节（v0.3 的发布与验收）和第 16 节（许可）；GPUI Kit 的[打包与分发](https://gpui-kit.com/zh-CN/docs/packaging)和[自动更新](https://gpui-kit.com/zh-CN/docs/auto-update)指南（2026-10-24 读）。
> 相关：[项目文件](project-format.md)第 11 节（示例项目）· [命令行与 Python](scripting.md)第 4.1、5.7 节 · [开发指南](../development.md)第 2.1 节（CI）

## 1. 范围

v0.3 是首个公开发布，要交付：

- macOS、Windows、Linux 的安装包，由 CI 构建，挂在 GitHub Releases 上；
- `mw-crystal`、`mw-crystal-io` 发布到 crates.io；
- 每个安装包附带第三方许可清单；
- 示例项目随应用附带。

不在 v0.3 里做：

- 应用商店。Mac App Store 要求沙盒，与写任意位置的项目目录、MCP 的本地套接字冲突；Microsoft Store、Flathub、Snap 以后再看（Flathub 可以顺带解决 Linux 的更新，第 9 节）。
- Linux 的 RPM、AppImage，Windows 和 Linux 的 ARM 版：先只做 x86_64，有用户要再加。
- Python 包发布到 PyPI：M19 待你确认（命令行与 Python 设计稿第 9 节），wheel 继续只在 CI 里构建。

## 2. 产物与应用身份

### 2.1 产物

| 平台 | 文件 | 是什么 |
|---|---|---|
| macOS（Apple 芯片和 Intel） | `Materials-Workbench-0.3.0-macos-universal.dmg` | 磁盘映像：`Materials Workbench.app` 和指向"应用程序"的替身。通用二进制（两种架构分别构建，`lipo` 合并），用户不必知道自己的芯片 |
| Windows x64 | `Materials-Workbench-0.3.0-windows-x64-setup.exe` | Inno Setup 安装程序，默认只装给当前用户，不要管理员权限 |
| Windows x64 | `Materials-Workbench-0.3.0-windows-x64.zip` | 便携版，解压即用 |
| Linux x86_64 | `materials-workbench_0.3.0_amd64.deb` | Debian / Ubuntu 包 |
| Linux x86_64 | `materials-workbench-0.3.0-linux-x86_64.tar.gz` | 便携版，带装进当前用户目录的 `install.sh` |
| 三个平台 | `mw-0.3.0-<目标>.tar.gz`（Windows 为 `.zip`） | 只有命令行 `mw` 和许可文件，给不装应用的服务器和 CI |
| 全部 | `SHA256SUMS` | 上面每个文件的 SHA-256 |

每个安装包里都有：应用 `materials-workbench`、命令行 `mw`（命令行与 Python 设计稿 4.1 节说过安装包要多带这个文件）、`LICENSE-MIT`、`LICENSE-APACHE` 和 `THIRD-PARTY-LICENSES.html`（第 7 节）。

**示例项目不另放文件**：它们已经编译进可执行文件（`mw_project_file::samples`，共 48 KB），第一次启动时复制到数据目录（项目文件设计稿第 11 节）。这在三个平台和便携版上一样工作，所以"示例项目放进安装包"已经做到，不必改成放在包里。

### 2.2 应用身份

| 项 | 值 | 说明 |
|---|---|---|
| 显示名 | Materials Workbench | |
| 可执行文件 | `materials-workbench`、`mw` | 已有 |
| macOS bundle identifier | `io.github.ember-moth.MaterialsWorkbench`（建议，第 10 节） | Apple 的标识只允许字母、数字、`-` 和 `.` |
| 项目的类型标识（UTI） | `io.github.ember-moth.MaterialsWorkbench.project` | 第 3.2 节 |
| Linux 应用 ID（`.desktop` 文件名、Wayland 的 app_id） | `io.github.ember_moth.MaterialsWorkbench`（建议） | freedesktop 和 Flathub 要求 `-` 换成 `_` |
| Debian 包名 | `materials-workbench` | |
| Windows 安装程序的 `AppId` | 写 `.iss` 时生成一个固定的 GUID，以后不再改 | 升级和卸载靠它认出同一个应用 |

标识定下后就不改：macOS 和 Windows 靠它们把新版认作旧版的升级。设置、最近项目和示例的位置不受影响：配置目录和数据目录用的是固定的名字 `materials-workbench`（`apps/workbench/src/config.rs`），与标识无关。

**版本号**：标签 `v0.3.0`、应用的版本和全部 crate 的版本一致。现在各 crate 的版本还是 `0.0.0`（`mw-crystal`、`mw-crystal-io` 已是 `0.3.0`，第 6 节）。发布时根 `Cargo.toml` 设 `[workspace.package] version`，各 crate 改为 `version.workspace = true`；项目文件里的 `writer`（`mw-project-file` 的 `WRITER`，取它自己的版本）随之变化，要重新生成示例项目；Python wheel 的版本取自 `mw-python` 的 Cargo 版本（`pyproject.toml` 里是 `dynamic`），自动跟着变。这一步放在发布前做（第 11 节），因为其他里程碑还在改示例项目。

## 3. macOS

### 3.1 应用包

```text
Materials Workbench.app/
└── Contents/
    ├── Info.plist
    ├── MacOS/
    │   ├── materials-workbench      应用（通用二进制）
    │   └── mw                       命令行（通用二进制）
    └── Resources/
        ├── AppIcon.icns
        ├── Project.icns             .mwproj 在访达里的图标（可选）
        ├── LICENSE-MIT
        ├── LICENSE-APACHE
        └── THIRD-PARTY-LICENSES.html
```

MCP 客户端的配置写完整路径：`/Applications/Materials Workbench.app/Contents/MacOS/materials-workbench mcp`（开发指南第 2 节的 Claude Code 写法同样适用）。`mw` 不在 `PATH` 上；以后可以像 VS Code 那样加"安装命令行工具"命令（在 `/usr/local/bin` 建链接，要管理员权限），v0.3 只在用户文档里写明路径，或者用单独的 `mw` 压缩包。

### 3.2 Info.plist

| 键 | 值 |
|---|---|
| `CFBundleIdentifier` | 第 2.2 节 |
| `CFBundleName` / `CFBundleDisplayName` | Materials Workbench |
| `CFBundleExecutable` | `materials-workbench` |
| `CFBundlePackageType` | `APPL` |
| `CFBundleShortVersionString` / `CFBundleVersion` | `0.3.0` / CI 的运行号 |
| `CFBundleIconFile` | `AppIcon` |
| `LSMinimumSystemVersion` | `11.0`：Rust 的 `aarch64-apple-darwin` 最低就是 11，GPUI 本身以 10.15 为目标；构建时设 `MACOSX_DEPLOYMENT_TARGET=11.0`，发布前至少在一台最旧的支持系统上验证 |
| `NSHighResolutionCapable` | `true` |
| `UTExportedTypeDeclarations` | 项目类型：标识见第 2.2 节，`UTTypeConformsTo` 为 `com.apple.package` 和 `public.composite-content`，扩展名 `mwproj`，描述 "Materials Workbench Project" |
| `UTImportedTypeDeclarations` | CIF（扩展名 `cif`，MIME `chemical/x-cif`，遵从 `public.plain-text`）和 POSCAR（扩展名 `vasp`、`poscar`）。这些不是我们的格式，只"导入"声明 |
| `CFBundleDocumentTypes` | 项目：`CFBundleTypeRole` 为 `Editor`，`LSHandlerRank` 为 `Owner`，`LSTypeIsPackage` 为 `true`；CIF 和 POSCAR：`Editor`，`LSHandlerRank` 为 `Alternate`（出现在"打开方式"里，不抢 VESTA 等已有程序的默认） |

`.mwproj` 登记为文档包后，访达里显示成一个文件、双击打开（项目文件设计稿第 3 节）。只叫 `POSCAR`、`CONTCAR` 的文件没有扩展名，不能按类型登记，拖进窗口或用"打开文件…"照常可以。

### 3.3 应用要补的代码

登记了文档类型，系统才会把文件交给应用，应用还得接住：

- **macOS**：双击或拖到 Dock 图标上的文件经 `App::on_open_urls`（`gpui-pre` 0.3.9 的 `src/app.rs:292`）以 `file://` URL 送来，应用没在运行时也一样（启动后送达）。`.mwproj` 目录按项目打开，其他文件走 `OpenFile` 的同一条路（导入当前项目）。
- **Windows 和 Linux**：文件以命令行参数传入（`materials-workbench 文件…`）。`main.rs` 现在只认第一个参数 `mcp`；其他参数改为要打开的路径，与上面同一个入口。
- 测试：界面集成测试直接调用这个入口（URL 列表、参数列表），检查项目进了边栏、文件导入了当前项目、无效路径给出通知。

### 3.4 签名与公证

两条路，**由你决定**（第 10 节）：

- **Developer ID 签名并公证**（要加入 Apple Developer Program，个人按年付费）：先签嵌套的 `mw`，再签 `.app`，都用 `--options runtime`（强化运行时）和时间戳；应用不需要特殊的 entitlement（不用 JIT，不在沙盒里）。DMG 签名后用 `notarytool` 提交公证、`stapler` 附上票据。CI 里要的机密：证书（p12 的 base64 和密码）、App Store Connect API 密钥（ID、issuer、私钥）。用户下载后直接打开，没有警告；以后的应用内更新也能用签名核对发布者（第 9 节）。
- **不签名**：整个包做 ad-hoc 签名（`codesign --force --deep --sign -`）。不做的话，Apple 芯片的 Mac 会说应用"已损坏"；做了以后第一次打开仍被拦下，用户要到"系统设置 → 隐私与安全性"里点"仍要打开"（macOS 15 起按住 Control 点"打开"不再能绕过）。用户文档要写明这一步。

### 3.5 磁盘映像

`hdiutil create -format UDZO`，内容是 `.app` 和指向 `/Applications` 的替身，v0.3 不做自定义背景。签名的话 DMG 也签名并公证（第 3.4 节）。最后在没装过开发工具的 Mac 上：挂载、拖进"应用程序"、第一次打开、打开示例项目、双击一个 `.mwproj`。

## 4. Windows

- **构建**：`cargo build --release --locked --bin materials-workbench -p mw-cli`，在 `windows-2025` 运行器上。
- **控制台窗口**：`main.rs` 没有 `#![windows_subsystem = "windows"]`，从开始菜单启动会多出一个控制台窗口。release 构建要加上（只在 `not(debug_assertions)` 时）。`materials-workbench mcp` 仍然可用：MCP 客户端用管道启动它，GUI 子系统的进程照样继承这些句柄；代价是在终端里手动运行时看不到日志，日志改写到文件或用 debug 构建。改完要在 Windows 上用 `apps/workbench/tests/mcp.rs` 的同一种方式（管道）验证。
- **图标与版本信息**：GPUI 从可执行文件的资源 1 加载窗口图标（`gpui-pre-windows` 0.3.9 的 `load_icon`），所以 `apps/workbench/build.rs` 要把 `.ico` 编为资源 1，并写入版本信息（文件属性里显示）。用一个只在 Windows 上生效的构建依赖（`winresource` 或已在依赖树里的 `embed-resource`，都是 MIT），许可检查会覆盖它。
- **安装程序**（Inno Setup 6，`packaging/windows/materials-workbench.iss`）：
  - `PrivilegesRequired=lowest`，`PrivilegesRequiredOverridesAllowed=dialog commandline`：默认装进 `%LOCALAPPDATA%\Programs\Materials Workbench`，不弹 UAC，以后的应用内更新也不需要提权；用户可以在对话框里改为给所有用户安装；
  - 开始菜单项；可选任务：桌面图标、把 `mw` 所在目录加进当前用户的 `PATH`、关联 `.cif` 和 `.vasp`（默认不勾）；
  - 卸载只删安装的文件，设置、示例副本和用户的项目留着；
  - 文件：两个可执行文件和三个许可文件。
- **`.mwproj`**：Windows 不能按扩展名关联文件夹，项目在资源管理器里仍是普通文件夹，从应用里打开（项目文件设计稿第 3 节已经这样写）。
- **便携版 ZIP**：同样的文件，不写注册表。
- **签名**：**由你决定**（第 10 节）。不签名时 SmartScreen 在下载和第一次运行时警告"Windows 已保护你的电脑"，用户点"更多信息 → 仍要运行"；签了名、积累下载量后警告消失。签名要先签两个 `.exe`，再签安装程序和它内嵌的卸载程序（Inno Setup 的 `SignTool` 设置）。
- 构建机上没有 Inno Setup 时用 Chocolatey 装（`choco install innosetup`）；运行器镜像里有没有自带，写工作流时核对。

## 5. Linux

- **构建机**：发布用的二进制在 `ubuntu-22.04` 上构建（glibc 2.35），能在更多发行版上运行；CI 的检查继续用 24.04。发布前在 22.04 上跑一遍测试，确认 GPUI 的依赖在那里也够新。
- **应用 ID**：Wayland 按窗口的 app_id 找对应的 `.desktop` 文件和图标。应用现在没有设置 `WindowOptions::app_id`（`gpui-pre` 0.3.9 的 `src/platform.rs:2546`），要设成第 2.2 节的 Linux 应用 ID，否则任务栏上的窗口没有图标、不归到启动器下。
- **`.desktop`**（`io.github.ember_moth.MaterialsWorkbench.desktop`）：`Exec=materials-workbench %F`，`Categories=Science;Chemistry;Physics;`，`MimeType=chemical/x-cif;`（系统认得这个类型时出现在"打开方式"里）。图标放 hicolor 的 PNG（多种尺寸）和 SVG。AppStream 元数据（`.metainfo.xml`）给软件中心用，v0.3 可以先不写，上 Flathub 时必须有。
- **`.deb`**：`/usr/bin/materials-workbench`、`/usr/bin/mw`、`/usr/share/applications/`、`/usr/share/icons/hicolor/`、`/usr/share/doc/materials-workbench/`（`copyright` 和许可清单）。`Depends` 由 `dpkg-shlibdeps` 从实际链接的库推出，再手动加 `libvulkan1`（wgpu 运行时才加载 Vulkan，`dpkg-shlibdeps` 看不到），`Recommends: mesa-vulkan-drivers`（没有显卡驱动时的软件渲染）。用 `dpkg-deb --root-owner-group --build` 生成，在干净的 Ubuntu 和 Debian 上装、启动、卸载。
- **`.tar.gz`**：`bin/`、`share/` 同样的布局加 `install.sh`，按 GPUI Kit 打包指南的做法装进 `~/.local`（核对发布方给的 SHA-256、只提取预期的文件、`.desktop` 里写绝对路径）。
- **`.mwproj`**：目录不能按扩展名关联，同 Windows。
- 开发指南第 1 节的 `-dev` 包是构建依赖，不是用户要装的；用户只需要图形会话和 Vulkan 驱动。

## 6. crates.io

只发布 `mw-crystal` 和 `mw-crystal-io`（路线图第 16 节"独立 crate"）。其余 crate 依赖 GPUI 或只是应用内部的拆分，继续 `publish = false`。

已做（2026-10-24）：

- **元数据**：`description`、`readme`、`documentation`（docs.rs）、`repository`（工作区统一设置）、`keywords`、`categories`（`science::materials`，`mw-crystal-io` 另加 `parser-implementations`；2026-10-24 核对过是 crates.io 现有的分类）、`license`（工作区继承）。两个名字在 crates.io 上都还没人用（同日查询）。
- **`publish`**：两个 crate 写 `publish = true`，不继承工作区的 `false`。
- **版本**：都是 `0.3.0`，随应用的版本走，1.0 之前小版本号的变化就是可能不兼容的变化。工作区依赖里写 `version = "0.3.0"`：发布时 Cargo 用版本代替路径。
- **README**：每个 crate 一份英文 README（crates.io 的读者），说明功能和一个例子；README 里的例子用 `#[cfg(doctest)]` 接进文档测试，和 crate 文档里同样的例子一起在 `cargo test --workspace` 里运行，不会过时。crate 文档里的设计稿引用改成 GitHub 上的链接，注明是中文。
- **许可文件**：crate 目录里的 `LICENSE-MIT`、`LICENSE-APACHE` 是指向仓库根目录的符号链接，打包时 Cargo 放进链接的目标。Windows 的检出默认不建符号链接（得到的是写着路径的文本文件），所以要从 macOS、Linux 或 CI 的 Linux 任务发布。
- **文档**：`RUSTDOCFLAGS="-D warnings" cargo doc --no-deps -p mw-crystal -p mw-crystal-io` 没有警告（修了两处：链接到私有函数、`write` 同时是函数和宏）。
- **依赖**：`mw-crystal` 只依赖 moyo（0.x，锁定小版本）、nalgebra、serde；`mw-crystal-io` 只依赖 `mw-crystal`。公开接口里没有 moyo 和 nalgebra 的类型，升级它们不会改变接口。
- **检查**：`cargo publish --dry-run -p mw-crystal -p mw-crystal-io` 通过。两个一起打包时，`mw-crystal-io` 对还没发布的 `mw-crystal` 的依赖由 Cargo 解析到同一批打出的包上；打包内容见 `cargo package --list`（源代码、README、两份许可、`mw-crystal-io` 的测试和测试数据）。CI 在 Linux 上做同样的打包和文档检查（开发指南 2.1 节）。

没写 `rust-version`：没有在较旧的工具链上测过（nalgebra 0.35 要求 1.89，工作区的文档写 1.92）。CI 加上最低版本的检查以后再写。

**发布顺序**：先 `mw-crystal` 再 `mw-crystal-io`（`cargo publish -p mw-crystal -p mw-crystal-io` 自动按依赖顺序）。第一次必须用 API 令牌在本机发布：crates.io 的 Trusted Publishing 只能给已存在的 crate 配置。之后在 crates.io 上把本仓库的 Release 工作流设为可信发布者，CI 用短期令牌发布（`rust-lang/crates-io-auth-action`，`id-token: write`），仓库里不存长期令牌。

## 7. 第三方许可清单

### 7.1 许可策略检查（已实现）

根目录的 `deny.toml` 列出允许的许可，`cargo deny check licenses` 检查全部依赖（含可选特性：Python 绑定和截图巡览），CI 每次运行。没有 GPL 类许可，只在 GPL 下提供的依赖会让检查失败；`A OR B` 的依赖只要一边允许就行（例如 `self_cell` 的 `Apache-2.0 OR GPL-2.0-only`）。

2026-10-24 的结果：全部通过。不是 MIT / Apache-2.0 的有：BSD、ISC、Zlib、Unicode-3.0、BSL-1.0、CC0、Unlicense、0BSD、MIT-0；MPL-2.0（`dwrote`，GPUI 在 Windows 上的字体；`option-ext`，来自 `dirs`），它的 copyleft 只管它自己的文件，源代码在 crates.io 上，清单里附上许可即可；CDLA-Permissive-2.0（`webpki-root-certs`，Mozilla 的根证书数据）；`Apache-2.0 WITH LLVM-exception`（`target-lexicon`，只在 PyO3 的构建脚本里用）。这和路线图第 16 节 2026-10-22 的核对一致。

### 7.2 清单的生成

| 做法 | 能做什么 | 不足 |
|---|---|---|
| **cargo-about**（Embark，MIT OR Apache-2.0） | 按依赖树收集每个 crate 自带的许可文本，相同的合并，crate 没带许可文件时用 SPDX 的标准文本，可以按目标平台选依赖，用模板生成 HTML 或 Markdown | 要安装新工具 |
| cargo-deny（已安装） | 检查许可（7.1 节在用），`cargo deny list` 列出每个 crate 的许可名 | 不给许可文本，MIT 和 BSD 要求随附的版权声明它拿不到 |
| 手写脚本（`cargo metadata` + 读 crate 目录里的文件） | 不装新工具 | 许可文件名五花八门，没带文件的要自己备 SPDX 文本，`AND` / `OR` 要自己处理：等于重写一遍 cargo-about |

**选 cargo-about**。做法：

- 根目录的 `about.toml`：接受的许可与 `deny.toml` 相同；`targets` 写发布的四个目标（`aarch64-apple-darwin`、`x86_64-apple-darwin`、`x86_64-pc-windows-msvc`、`x86_64-unknown-linux-gnu`），每个平台的清单只含该平台实际编进去的 crate；
- `about.hbs` 模板生成 `THIRD-PARTY-LICENSES.html`：按许可分组，每组列出 crate 和版本，再附许可全文；
- crate 之外的内容在模板里手写一段：GPUI Kit 资源里的 Lucide 图标（ISC，`gpui-kit-assets` 的 `LICENSE-LUCIDE`，要确认 cargo-about 是否已收进来）；元素数据（IUPAC 标准原子量等，是科学事实，注明来源）；
- Release 工作流在每个平台上运行 `cargo about generate`，结果放进安装包（第 3.1、4、5 节的位置）。以后在"关于"对话框里加一个打开它的入口。

**状态**：cargo-about 本机没有装，要从 crates.io 下载（`cargo install cargo-about --locked`），**等你同意**后再写这两个文件并在本机生成、核对。

## 8. GitHub Releases

新的工作流 `.github/workflows/release.yml`（还没写）：

- **触发**：推送 `v*` 标签；`workflow_dispatch` 只构建、不建 Release，用来试打包。
- **任务**：
  1. `check`：标签与工作区版本一致，`CHANGELOG.md` 有这个版本的一节；
  2. `build`（矩阵：macOS 15 构建两种架构再合并、Windows 2025、Ubuntu 22.04）：release 构建 → 生成许可清单 → 打包（脚本放在 `packaging/<平台>/`，本机也能运行）→ 有签名机密时签名（和公证）→ 冒烟测试（`mw info` 一个示例文件；应用以后加 `--version`）→ 上传产物；
  3. `release`：下载全部产物，算 `SHA256SUMS`，`gh release create --draft` 建草稿，正文取 `CHANGELOG.md` 的那一节。只有这个任务有 `contents: write` 权限；
  4. `crates`：草稿发布之后手动触发，在受保护的 `release` 环境里发布到 crates.io（第 6 节）。
- **人工把关**：草稿由你检查后发布；发布前按 GPUI Kit 打包指南的清单在干净的机器上验证每种格式：安装、启动、打开示例项目、双击 `.mwproj`（macOS）、从上一版升级（v0.3 是第一版，从 v0.4 起）、卸载。
- **机密**：只有选了签名才需要（第 3.4、4 节）；放在 `release` 环境里，构建 PR 的工作流拿不到。
- 推送前问你；工作流的结果要推送后才知道。

## 9. 自动更新

GPUI Kit 没有统一的更新服务，更新方式按安装来源不同（自动更新指南）。几种做法：

| 做法 | 怎么做 | 好处 | 代价 |
|---|---|---|---|
| **A. 只检查，不安装** | 启动时（至多每天一次）和"检查更新"命令查询 GitHub Releases 的最新版本，有新版就通知，点开下载页 | 简单；不替换文件，不需要权限，不依赖签名；失败不影响使用 | 用户自己下载安装；会联系 `api.github.com`（暴露 IP），要在设置里能关 |
| **B. 应用内下载并安装** | 按安装来源：macOS 替换整个 `.app`；Windows 下载新的安装程序静默运行（同一个 `AppId` 原地升级）；Linux 便携版替换文件；`.deb` 只提示 | 一键更新 | 每种来源都要实现和测试（下载中断、无写权限、文件被占用、回退）；必须核对发布者：平台签名（第 3.4、4 节），或自己的签名（minisign / Ed25519，私钥在 CI 机密里，公钥编进应用）。只核对同一位置的 `SHA256SUMS` 不够 |
| **C. Sparkle / WinSparkle** | macOS 用 Sparkle、Windows 用 WinSparkle，读签名的 appcast | 成熟，有增量更新和标准界面 | 两套 Objective-C / C 框架要打包和接到 GPUI 上；不管 Linux；Sparkle 基本要求 Developer ID 签名 |
| **D. 交给包管理器** | Homebrew cask、winget、Flathub 或 apt 仓库 | 应用里不写更新代码，用户用熟悉的工具 | 每个渠道要提交和维护；Homebrew 和 winget 更欢迎签过名的应用 |

建议：**v0.3 做 A**（加设置开关），签名定下之后的版本再做 B（先 Windows 安装版和 Linux 便携版，macOS 等 Developer ID），D 视用户需要逐个加。A 的检查默认开还是关、第一次启动要不要问，也**由你决定**（第 10 节）。

## 10. 要你决定的

1. **macOS 签名与公证**：
   - (a) 加入 Apple Developer Program，用 Developer ID 签名并公证；
   - (b) 不签名，用户第一次打开时在系统设置里放行。

   建议 (a)：没有公证，macOS 15 起放行要进系统设置操作，对首次公开发布的第一印象很差；以后 macOS 的应用内更新（第 9 节 B、C）也要靠它。如果暂时不想付费，v0.3 可以先 (b)，用户文档写明放行步骤。
2. **Windows 签名**：
   - (a) 不签名，SmartScreen 警告，用户点"仍要运行"；
   - (b) SignPath Foundation 给开源项目的免费签名：要求项目已经以要签的形式发布过、全部组件开源（本项目符合）、签名在它们的服务上进行（CI 集成），证书颁发给 SignPath Foundation，所以显示的发布者是它而不是你；
   - (c) Azure Artifact Signing（原 Trusted Signing）：按月付费，个人要做身份验证，只对部分国家和地区开放，先查它的支持列表；
   - (d) 向 CA 买 OV 证书：私钥必须放在硬件或云 HSM 里，CI 要接云签名服务，每年几百美元。

   建议 v0.3 先 (a)，正式发布后申请 (b)（它要求先发布过）；(c) 在你所在地区可用的话也可以，发布者显示的是你自己。
3. **自动更新**：第 9 节的 A、B、C、D。建议 v0.3 只做 A，并决定检查默认开还是关（建议默认开、在设置里可关，第一次检查前不另外询问；或者默认关、第一次启动时问一次）。
4. **小项**（有建议，你不提就按建议做）：
   - 标识：macOS `io.github.ember-moth.MaterialsWorkbench`，Linux `io.github.ember_moth.MaterialsWorkbench`（第 2.2 节）。有自己的域名的话可以换成它，发布后就不能再换；
   - 应用图标：要设计一个（`.icns`、`.ico`、各尺寸 PNG 和 SVG）。你可以提供，或者我先做一个简单的，你再定；
   - Linux 先只做 `.deb` 和 `.tar.gz`（第 1 节）。

## 11. 实施顺序

| 阶段 | 内容 | 状态 |
|---|---|---|
| 1 | 本设计稿；crates.io 元数据、README 与文档、dry run；`deny.toml` 和 CI 里的许可检查、打包检查 | 已做（2026-10-24），CI 的新任务要推送后才知道结果 |
| 2 | cargo-about 的配置和模板，本机生成并核对许可清单 | 等你同意安装 cargo-about |
| 3 | 应用侧：打开文件的入口（第 3.3 节）、Windows 的子系统与图标资源（第 4 节）、Linux 的 app_id（第 5 节）、`--version`；选了 A 的话加更新检查 | 不需要决定，可以开始；图标先用占位 |
| 4 | 打包脚本 `packaging/{macos,windows,linux}/`，先在本机打出 macOS 的 `.app` 和 `.dmg`（ad-hoc 签名） | 不需要决定 |
| 5 | `release.yml`，用 `workflow_dispatch` 试打包 | 要推送，推送前问你 |
| 6 | 签名和公证接进工作流；干净机器上的验证；`CHANGELOG.md`、用户文档、演示视频（你来录） | 等第 10 节的决定 |
| 7 | 发布：统一版本号、重新生成示例项目、打标签、检查草稿后发布、第一次手动发布到 crates.io | 验收之后（路线图第 7 节） |
