# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Materials Workbench：用 Rust 编写的开源、跨平台材料科学桌面工作台。第一阶段做晶体学（类似 VESTA），之后是 XRD、CALPHAD 相图、显微组织和实验室数据；所有能力都要能被 AI 调用（MCP Server 和内置助手）。当前处于 v0.1，正按里程碑把模拟（mock）逐个换成真实功能。

## 文档是设计的依据

动手前先读对应的文档，设计需要改时先改设计稿：

- `docs/architecture.md`：crate 划分、依赖方向、运行时结构；**第 9 节是模拟清单，第 10 节是 v0.1 里程碑 M1–M6 及状态**。
- `docs/development.md`：命令、代码约定（图标、文字、命令、模拟）、测试分层、交付流程、提交规范。
- `docs/roadmap.md`：各版本的功能、算法与验收标准。
- `docs/design/`：
  - `crystallography-model.md`：数据模型与编辑语义；
  - `viewport-rendering.md`：视口技术选型与 GPUI 源码核查结论；
  - `ui-shell.md`：界面布局与术语表；
  - `ai-integration.md`：MCP、内置助手与模型服务层。

文档用中文写，API 名、crate 名、文件名保留原文。实现与设计不一致时，要修订设计稿，不让两者分叉。

## 常用命令

```bash
cargo run --bin materials-workbench          # 运行应用
cargo test --workspace                       # 全部测试
cargo test -p mw-crystal-io --test fixtures  # 单个集成测试文件
cargo test -p mw-structure rutile_apical_bond_and_angle   # 单个测试（按名称过滤）
cargo test -p mw-structure ui_tests                       # 界面集成测试
cargo clippy --workspace --all-targets
cargo fmt --all
```

- **截图巡览**：改界面后自查用。依次切换主要界面并渲染成 PNG，然后自动退出，不需要屏幕录制权限。巡览的步骤在 `apps/workbench/src/screenshot.rs` 的 `TOUR` 里；点选、测量和选中要删除的原子都发送真实的鼠标事件（`Pointer` 动作），走的是和用户一样的拾取路径；编辑步骤用 `StructureWorkspace::execute` 执行领域命令，再撤销回原样。

  ```bash
  MW_SCREENSHOT_DIR=screenshots cargo run --bin materials-workbench --features screenshot
  ```

  窗口被遮挡或切到后台时系统会降低刷新，会截到旧画面：运行期间让窗口保持在前台，看图时先确认相邻几张没有完全相同的。
- **无窗口渲染一帧**：`cargo run -p mw-crystal-render --example snapshot -- snapshot.png 1000`
- **视口性能基准**：`VIEWPORT_POC_BENCH=1 cargo run -p viewport-poc`，会打开窗口，约 1 分钟后自动退出；包括带键的负载和拾取延迟。窗口失去焦点的档位（输出里 `window_lost_focus=true`）数据作废。
- **模型服务示例**：`cargo run -p mw-llm --example chat -- "问题"` 发一轮对话并打印流式事件，配置全部来自 `MW_LLM_*` 环境变量（见 `docs/development.md` 第 2 节）。它会发真实请求并计费，运行前先问用户；本机的 Ollama 不花钱（`MW_LLM_PROVIDER=ollama`）。
- **COD 语料统计**：测试默认忽略，需要本地语料，不随仓库提交。

  ```bash
  MW_COD_DIR=~/cod/cif cargo test -p mw-crystal-io --release --test cod_corpus -- --ignored --nocapture
  ```

工具链：Rust 1.92 或更新（本仓库在 1.97 上开发），edition 2024；macOS 需要 Xcode。debug 构建里 GPUI 和 wgpu 以 `opt-level = 3` 编译（见根 `Cargo.toml`）。

## 架构要点

### crate 分层

工作区用 `members = ["crates/*", "apps/*"]`：在 `crates/` 下新建目录，它就自动成为工作区成员。依赖只能从上往下：

- **`apps/workbench`**：只做组装，包括窗口外壳、全局命令、命令面板、设置和图标资源。
- **能力 crate（依赖 GPUI）**：
  - `mw-project`：项目目录和左侧边栏；
  - `mw-structure`：打开的结构，包括文档（包着 `mw_crystal::CrystalDocument`）、视口浮层、右侧面板、对话框和命令；编辑在 `src/editing.rs`，所有修改都经 `StructureWorkspace::execute` 执行领域命令，再更新字段、场景和选区。

  两者互不引用，由外壳通过事件转接，例如 `ProjectSidebarEvent::OpenStructure`。
- **`mw-viewport`**：把渲染结果放进 GPUI，负责相机交互、异步出帧、拾取（`ViewportEvent::Clicked` / `Hovered`）、悬停高亮和投影。它不知道晶体是什么。
- **纯 Rust、不依赖 GPUI 的 crate**：
  - `mw-crystal`：模型、对称操作、原子展开、派生数据（含对称性分析：空间群识别、位点对称性、Wyckoff 位置、晶胞约束；成键，见 `src/bonds.rs`；化学式、近邻）、`AtomRef` 与测量的几何、元素表、内置示例；编辑命令（`src/edit.rs`）和带撤销栈的 `CrystalDocument`（`src/document.rs`）；符号表和对称性搜索来自 moyo，只在 `src/tables.rs` 里调用；
  - `mw-crystal-io`：CIF 1.1 子集解析和位点整理（吸附到特殊位置、合并混合占位），产出带行号的诊断；
  - `mw-crystal-render`：wgpu 离屏渲染器，画球、键、线段和高亮圈，并用 ID 纹理回答"这个像素上是第几个球或键"；
  - `mw-automation`：能力注册表（`ai-integration.md` 第 4 节）。每个能力有英文说明、输入输出 Schema、副作用等级和处理函数，调用的是 `mw-crystal` 的领域命令；Schema 由 `schema_struct!` 从 Rust 类型和字段文档注释生成，测试用 `mw-llm` 的检查函数确认它们在严格子集内。`StructureEntryId` 定义在这里，`mw-project` 重新导出；
  - `mw-llm`：内置助手的模型服务客户端。`ChatProvider` 接口下有 Anthropic Messages、OpenAI Chat Completions、OpenAI Responses 三个后端；HTTP 跑在专用线程的 tokio runtime 上，返回的流可以由 GPUI 的执行器直接轮询。目前还没有使用者，以后由 `mw-assistant` 依赖（`ai-integration.md` 6.2 节）。它的测试只用手写的 SSE 数据和本机回环服务器，不访问外网。

  这些 crate 都要能在命令行和测试里单独使用。纯计算一律放在这一层并在这里写单元测试。
- **按能力分 crate**：一个能力的模型、视图、命令和对话框放在一起，不按"视图 / 模型"分目录。只有一个使用者的东西不提前抽成共享 crate。

### 晶体学模型

- **真源**是不对称单元加对称操作：整数矩阵加有理平移，并做群封闭检查。
- **派生数据**由真源计算：等效原子、`AtomRef`（位点 + 轨道成员 + 晶胞偏移）、位点对称性、键（共价半径之和 + 容差，跨周期边界，金属之间默认不成键）。
- 能力、选区和测量都引用稳定 ID（`StructureEntryId`、`SiteId`、`AtomRef`），不用列表下标。
- 编辑分对称模式和 P1 模式，都是可序列化的命令（`mw_crystal::Edit`），带精确的逆命令，进入 `CrystalDocument` 的撤销栈（M4）。命令只指向整个位点；指向单个原子的请求先经 `target_sites` 换算，对称模式下只改部分等效原子会被拒绝并给出引导。界面、能力注册表（`mw-automation`）和以后的脚本调用的是同一批命令。

### 视口

- 视口在 wgpu 里离屏渲染（impostor 球和圆柱，正交投影，MSAA 4×），回读后用 `Window::paint_image` 画进 GPUI。
- stock GPUI 在任何平台上都没有零拷贝的表面，理由见 `viewport-rendering.md` 第 3 节。
- 拾取用单独的 1× `R32Uint` ID 通道，只在拾取时、相机或场景变过之后才重画（`viewport-rendering.md` 5.4 节）。
- `mw-structure` 决定画什么和叠什么：
  - 原子颜色与半径、键、晶胞边和测量线在 `src/scene.rs` 组成场景，第 i 个球就是 `derived.atoms[i]`；
  - 选区（位点列表和视口共用）在 `src/selection.rs`，测量模式在 `src/measure.rs`；
  - 测量标注等浮层的位置用 `Viewport::project` 计算。

### 命令与快捷键

- 每个 crate 用 `gpui_kit::actions!` 定义动作，在自己的 `bind_keys` 里注册快捷键。
- 按键上下文有两个：`Workbench`（外壳）和 `StructureWorkspace`（只在结构工作区内有效的快捷键）。
- 处理器写在拥有该命令的视图上。按钮、快捷键和命令面板都派发同一个动作。
- 撤销和重做是 `Undo` / `Redo`（⌘Z / ⇧⌘Z，绑定在 `StructureWorkspace` 上下文里；输入框更深的上下文里是撤销文字）。
- 结构相关的命令加进 `mw_structure::palette_commands()`，外壳命令加进 `apps/workbench/src/palette.rs`；关键词写英文名和拼音。
- 命令面板确认后，先关闭面板，把焦点还给原来的位置，再用 `window.defer` 派发动作。

### 打开文件

- 对话框、⌘O 和拖放都走 `OpenFile`。
- 在后台任务里用 `mw_structure::import::OpenedFile::read` 读取文件，再交给 `mw_crystal_io::cif::read` 解析。
- 成功的数据块加入"导入的文件"项目（`ProjectCatalog::add_imported`）。
- 诊断在 `mw-crystal-io` 里是数据（`DiagnosticKind`），文字由 `mw-structure/src/diagnostics.rs` 按当前语言翻译。

## 必须遵守的约定

- **不要凭印象写 GPUI 或 GPUI Kit 的 API。** 先到 `~/.cargo/registry/src/*/` 下的 `gpui-pre-*`、`gpui-component-*`、`gpui-base-*`、`wgpu-*` 里查真实签名，版本以 `Cargo.lock` 为准（GPUI Kit 0.8.0、`gpui-pre` 0.3.9、wgpu 29）。
  - 代码里统一用 `use gpui_kit::*`，组件从 `gpui_kit::component` 取；
  - 写界面前先加载 `gpui-kit` 和 `gpui-kit-design-guides` 两个技能。
- **文字**：
  - 每个有界面的 crate 都有自己的 `locales/ui.yml`，同时写 `zh-CN`（默认）和 `en`（回退）；代码里用 `tr!`；
  - 完整的句子放在一个键里，参数用 `%{name}`，不拼接翻译片段；
  - 领域 crate 不放界面文字。
- **图标**：
  - 组件的 `IconName` 只是 Lucide 的子集；
  - 需要子集之外的图标时，用 `gpui_kit::assets::IconName as Lucide` 并包成 `Icon::new(...)`；
  - 同时必须把它登记进 `apps/workbench/src/assets.rs` 的列表，否则运行时找不到资源，图标显示为空白。
- **样式**：
  - 颜色只用 `cx.theme()` 的语义色，唯一例外是元素颜色等数据颜色（`mw-structure/src/scene.rs`）；
  - 尺寸用 rem 辅助函数；写像素值时必须加注释说明原因。
- **模拟**：
  - 模拟代码用 `Mock:` 开头的注释标出，`rg "Mock:"` 可以列出全部；
  - 用户能触发的未实现操作要调用 `not_implemented` 给出提示；
  - 替换掉模拟后，同步更新 `docs/architecture.md` 第 9 节的模拟表和第 10 节的里程碑状态。
- **`screenshot` feature** 直接开启 `gpui-pre` 和 `gpui-pre-macos` 的 `test-support`。不要改成 GPUI Kit 自己的 `test-support`：它会让 Cargo 解析可选的 GPUI Fast 后端，其 `cocoa` 版本与 GPUI 冲突。界面集成测试（`mw-structure/src/ui_tests.rs`，`#[gpui_kit::test]`）同样以 dev-dependency 直接开启 `gpui-pre` 的 `test-support`，所以用不了 `gpui_kit::test` 的交互辅助，改用 GPUI 的 `VisualTestContext`：聚焦字段、`simulate_input` 打字、`simulate_keystrokes` 按键。
- **测试数据**放在使用它的 crate 的 `tests/fixtures/` 里，每个文件开头写明内容和来源；取自数据库的文件要注明条目号和许可。

## 交付与提交

- **交付顺序**：读设计稿 → 在不依赖界面的 crate 里实现并写测试 → 接到界面上、删掉模拟 → 更新文档 → 运行 `cargo fmt --all`、clippy、`cargo test --workspace` 和截图巡览。
- **提交信息**用 Conventional Commits（`feat:`、`fix:`、`docs:`、`refactor:`、`test:`、`chore:`）。标题和正文都用英文，标题用祈使语气；文档跟随它描述的代码一起提交。
- **只在用户要求时提交，不要推送。** 仓库里经常有多个会话并行，各自在主检出或 worktree 里工作：提交时只暂存自己改的文件，不要带上别的会话未提交的改动。
- 用户用中文交流。
