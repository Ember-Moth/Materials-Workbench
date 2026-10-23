# 架构

> 状态：v0.1 框架已搭好，M1 CIF 导入、M2 对称性、M3 渲染（键、晶胞边框、拾取、测量）、M4 基础编辑与能力注册表、M5 中的导出（CIF、POSCAR、扩展 XYZ）、M5b 转为 P1 与超胞、M6 持久化（项目文件、最近项目、设置）已实现，v0.1 的里程碑都已完成 · 2026-10-22
> 相关设计：[界面外壳](design/ui-shell.md) · [晶体学数据模型](design/crystallography-model.md) · [项目文件](design/project-format.md) · [3D 视口](design/viewport-rendering.md) · [AI 集成](design/ai-integration.md)
> 开发流程与约定见 [开发指南](development.md)；版本规划见 [产品路线图](roadmap.md)。

## 1. 目标

Materials Workbench 是一个用 Rust 编写的现代化、开源、跨平台的材料科学桌面工作台。第一阶段做晶体学：查看、测量、编辑晶体结构，再导出给下游计算。之后逐步加入 XRD、相图和显微组织分析。

当前状态：界面按设计稿实现；可以打开 CIF 文件（M1），对称操作集、原子展开、成键和对称性分析（空间群识别、位点对称性与坐标约束、Wyckoff 位置、晶胞约束，导入时吸附到特殊位置并合并混合占位，M2）是真实计算的；视口画出原子、键和晶胞边框，可以点选原子、测量距离和键角（M3）。晶胞参数、元素、占位率与混合占位、删除位点、P1 下的坐标可以编辑并撤销，同一批命令也注册在能力注册表里（M4）；结构可以导出为 CIF、POSCAR 和扩展 XYZ，CIF 会写回源文件里模型不保存的字段（M5）。项目是磁盘上的 `.mwproj` 目录，保存结构、测量、来源记录、编辑记录和视图状态，关闭再打开完整恢复；最近项目和设置跨重启保存（M6）。结构可以转为 P1（原结构上执行、可撤销，选区和测量跟着原子走），可以生成超胞，作为带来源记录的派生结构加进项目（M5b）。v0.2 的实验性只读 MCP Server 已提前做出原型（`mw-mcp`）：MCP 客户端可以打开 CIF 文件、查询结构、取得导出文本。第 9 节列出所有模拟，第 10 节给出替换它们的顺序。

架构上为后续模块预留的位置：XRD、热力学、显微组织各自是"计算 crate（无界面）+ 界面能力 crate"的组合，与晶体学模块并列，共享项目系统、命令与撤销、视口与图表、国际化。计划中的 crate 见[产品路线图](roadmap.md)第 8–10 节。

## 2. 仓库结构

```text
materials-workbench/
├── apps/
│   ├── workbench/            桌面应用（可执行文件 materials-workbench）
│   └── viewport-poc/         视口基准测试工具
├── crates/
│   ├── mw-crystal/           晶体结构模型（纯 Rust，无界面）
│   ├── mw-crystal-io/        结构文件读写（纯 Rust，无界面）：CIF 导入；CIF、POSCAR、扩展 XYZ 导出
│   ├── mw-crystal-render/    wgpu 离屏渲染器（不依赖 GPUI）
│   ├── mw-viewport/          3D 视口组件（GPUI + 渲染器）
│   ├── mw-project/           项目：项目目录与左侧边栏
│   ├── mw-project-file/      项目文件的格式与读写、示例项目（纯 Rust，无界面）
│   ├── mw-structure/         结构工作区：文档、视图、面板、对话框、命令
│   ├── mw-automation/        能力注册表：GUI、脚本、MCP、助手共用的能力（纯 Rust，无界面）
│   ├── mw-mcp/               实验性 MCP Server（纯 Rust，无界面；可执行文件 mw-mcp）
│   └── mw-llm/               内置助手的模型服务客户端（纯 Rust，无界面）
└── docs/
    ├── architecture.md       本文
    ├── roadmap.md            产品路线图
    ├── development.md        开发指南
    └── design/               各模块设计稿
```

## 3. crate 与依赖方向

依赖只能从上往下，同层之间不互相依赖：

```text
apps/workbench ─────────┬──────────────────┐
                        ▼                  ▼
                   mw-project         mw-structure
                        │             │          │
                        ▼             ▼          ▼
                   mw-crystal-io ◄────┘     mw-viewport
                        │                        │
                        ▼                        ▼
                    mw-crystal          mw-crystal-render
```

`mw-project` 和 `mw-structure` 也直接使用 `mw-crystal` 的类型，图中省略了这两条边。`mw-project-file`（项目文件的读写）依赖 `mw-crystal` 和 `mw-crystal-io`，`mw-project`、`mw-structure` 和外壳使用它，以后 MCP 的无界面模式也用它读写项目。`mw-automation`（能力注册表）依赖 `mw-crystal` 和 `mw-crystal-io`（导入诊断与导出文本）；`mw-project` 依赖它，只为重新导出 `StructureEntryId`。`mw-mcp`（MCP Server）依赖 `mw-automation`、`mw-crystal-io` 和官方 MCP SDK `rmcp`，不依赖 GPUI，也没有别的 crate 依赖它；以后内置助手也从注册表取能力。`mw-llm` 不依赖其他 crate，目前也没有使用者；以后由内置助手 `mw-assistant` 依赖（[AI 集成设计稿](design/ai-integration.md)第 3 节）。

| crate | 职责 | 依赖 GPUI |
|---|---|---|
| `mw-crystal` | 晶体结构的数据类型（晶胞、对称操作集、位点、容差）、原子展开与派生数据（含 `AtomRef`、成键、化学式和近邻）、测量的几何、对称性分析（空间群识别、位点对称性、Wyckoff 位置、晶胞约束；符号表和对称性搜索来自 moyo，只在 `tables.rs` 里调用）、编辑命令与带撤销栈的文档（`CrystalDocument`）、元素表，以及内置示例 | 否 |
| `mw-automation` | 能力注册表（[AI 集成设计稿](design/ai-integration.md)第 4 节）：每个能力有名称、英文说明、由 Rust 类型生成的输入输出 JSON Schema、副作用等级、可用性和处理函数；结构的查询与编辑、撤销与重做已注册。处理函数通过 `Host` 拿到文档，调用 `mw-crystal` 的领域命令 | 否 |
| `mw-mcp` | 实验性只读 MCP Server（[AI 集成设计稿](design/ai-integration.md)第 5 节）：把注册表里只读、无界面可用的能力映射成 MCP 工具（带只读注解和输出 Schema），加上自己的 `file_open`（只能打开启动时允许的目录里的 CIF 文件）；每个打开的结构的 CIF 文本是资源；来自文件的文本标为不可信。可执行文件 `mw-mcp` 走 stdio，同时支持 `initialize` 和 2026-07-28 的无状态生命周期 | 否 |
| `mw-crystal-io` | 解析结构文件（CIF 1.1 子集），整理位点（吸附到特殊位置、合并混合占位、去掉重复行），产出结构和带源文件行号的导入诊断，并保留模型不存的字段；把结构写成 CIF、POSCAR 和扩展 XYZ 文本。诊断和导出错误是数据，文字由界面翻译 | 否 |
| `mw-crystal-render` | 离屏渲染球、键、线段和高亮圈并回读像素；按需渲染 ID 纹理回答"鼠标下是什么"；只依赖 wgpu | 否 |
| `mw-viewport` | 把渲染结果放进 GPUI：相机交互、异步出帧与拾取、悬停高亮、3D 坐标投影到屏幕 | 是 |
| `mw-project-file` | 项目文件（[项目文件设计稿](design/project-format.md)）：清单、结构文件、来源记录、视图状态的类型；读写 `.mwproj` 目录（原子写入、清单最后写、清理不再引用的文件、旧格式迁移）；导入文件的副本与哈希；随应用附带的示例项目 | 否 |
| `mw-project` | 打开着的项目（清单、目录、导入或派生后还没保存的结构、保存进度）和左侧边栏视图 | 是 |
| `mw-structure` | 一个打开的结构：顶栏、视口与浮层、右侧面板、对话框、命令；选区与测量；打开文件后的通知与错误列表；导出对话框与写文件；给出要保存的内容和视图状态 | 是 |
| `apps/workbench` | 窗口外壳：组装边栏与工作区、全局命令、命令面板、主题与语言；项目的新建、打开、保存、关闭与未保存时的提示；最近项目、设置和会话的持久化 | 是 |
| `mw-llm` | 内置助手的模型服务客户端：`ChatProvider` 接口和项目自己的对话类型；Anthropic Messages、OpenAI Chat Completions（含兼容服务和本地模型）、OpenAI Responses 三个后端；工具 Schema 子集检查与参数校验；能力声明与厂商档案。HTTP 跑在专用线程的 tokio runtime 上，流可以由任何执行器轮询 | 否 |

**规则**：

- **按能力分 crate，而不是按"视图 / 模型 / 对话框"分目录**（GPUI Kit Coding Guides）。一个能力的模型、视图、命令和对话框放在同一个 crate 里。
- **领域 crate 不依赖界面**。`mw-crystal`（含对称性分析）和 `mw-crystal-io` 都要能在 CLI 或测试里单独使用。
- **渲染器不依赖 GPUI**，见视口设计稿 5.1 节。
- **能力之间不互相依赖**。`mw-project` 和 `mw-structure` 互不引用，由外壳通过事件转接（见第 4 节）。
- 只有一个能力在用的东西不提前抽成共享 crate；有第二个使用者时再抽。

## 4. 运行时结构

```text
Workbench（窗口根视图，apps/workbench）：打开着的工作区、当前项目、会话（最近项目）
├── ProjectSidebar（mw-project）：ProjectCatalog（打开着的项目，每个项目的清单、目录、尚未保存的结构）
├── StructureWorkspace（mw-structure），没有打开结构时为空状态
│   ├── StructureDocument：CrystalDocument（结构、派生数据、测量、撤销栈）+ 名称 + 来源记录 + 导入诊断 + 以前的编辑记录
│   ├── Selection：位点列表和视口共用的选区
│   ├── Viewport（mw-viewport）：渲染器、相机、出帧与拾取流水线、悬停高亮
│   └── 各字段的 InputState
└── CommandState：命令面板的查询与选择状态
```

**打开项目的流程**（[项目文件设计稿](design/project-format.md)）：

1. 启动时读配置目录里的 `state.json`，在后台按顺序读上次打开着的项目的清单和视图状态（`mw_project_file::read_manifest`）；第一次启动先把随应用附带的示例项目装到数据目录。`打开项目…`（⇧⌘O）、最近项目和拖进窗口的 `.mwproj` 文件夹走同一条路。
2. 每个项目加入 `ProjectCatalog`，会话里的 `StructureEntryId` 这时分配；已经打开着的（同一路径或同一项目 ID）不重复打开。
3. 显示该项目上次打开的结构。

**打开结构的流程**：

1. 用户点击边栏里的结构，`ProjectSidebar` 发出 `ProjectSidebarEvent::OpenStructure(id)`。
2. 本次会话打开过的直接切换回去（编辑、撤销历史和视角都在）。否则：导入或派生后还没保存的结构直接用内存里的内容；已经在磁盘上的，在后台读 `structures/<id>.json`，导入的结构再读源文件副本、重新解析出数据块（诊断和 CIF 保留字段）。
3. `Workbench` 用这些构造 `StructureDocument` 和 `StructureWorkspace`，应用项目里保存的视图状态（相机、显示模型、面板），把焦点交给它，并观察它的变化来更新保存状态。

**保存的流程**（⌘S、右键菜单、命令面板）：

1. 未命名项目和"另存为"先用系统保存面板选位置；已存在的路径不覆盖。
2. 界面线程上收集要写的内容：有编辑的工作区给出 `StructureFile` 和此刻的 `Checkpoint`，还没写到磁盘的结构和源文件副本来自项目目录模型，视图状态来自所有打开的工作区。
3. 后台任务里 `mw_project_file::save` 写目录：源文件副本、改过的结构、最后清单，再清理不再引用的文件。
4. 回到界面线程：项目目录模型记下保存（写的过程中又新增的结构仍是未保存），各工作区按各自的 `Checkpoint` 标记保存点，最近项目更新，顶栏和边栏的状态随之刷新。
5. 关闭项目、退出应用（⌘Q、关窗口）时有未保存的更改就先问；退出时写各项目的 `view.json` 和配置目录里的 `state.json`。

**打开文件的流程**：

1. 边栏、⌘O、命令面板和空状态里的"打开文件…"都派发 `OpenFile`；`Workbench` 用 `prompt_for_paths` 弹出系统文件对话框。拖进窗口的文件由根视图的 `on_drop::<ExternalPaths>` 接收，走同一条路。
2. 后台任务里，`mw_structure::import::OpenedFile::read` 读文件，用 `mw_crystal_io::cif::read` 解析。
3. 回到界面线程，每个成功的数据块连同文件的字节加入当前项目（`ProjectCatalog::add_imported`，记下来源：文件名、路径、SHA-256、数据块），没有当前项目时新建一个未命名项目，然后打开第一个。保存项目时文件原样复制进项目的 `sources/`。
4. `OpenedFile::notify` 告诉用户结果：顺利导入时不提示（打开的结构本身就是反馈）；有诊断时发通知，诊断在"诊断"标签页里；导入失败时发错误通知，通知上的"查看错误…"打开带行号的错误列表。

**导出的流程**（`mw-structure/src/export.rs`）：

1. 顶栏的"导出…"、⌘E 和命令面板都派发 `ExportStructure`；工作区打开导出对话框，列出 CIF、POSCAR、扩展 XYZ。结构装不进的格式（部分占位、未知元素）置灰，并写明原因。默认选中本次会话上次用的格式。
2. 确认后用 `prompt_for_new_path` 弹出系统保存面板，建议的文件名是结构名加扩展名，目录是上次导出的目录。
3. 后台任务里用 `mw_crystal_io` 的 `cif::write`、`poscar::write` 或 `xyz::write` 生成文本并写文件。导入的结构把源数据块交给 CIF 写出（`ImportedBlock::write_options`），描述性和未识别的字段随之写回（晶体学设计稿第 6 节）。
4. 写好后发通知，带"在访达中显示"；写入失败时发错误通知；结构修改过、有字段没写回时发警告通知并列出这些字段。

**视口出帧**：`StructureWorkspace` 把文档的显示原子、显示键、晶胞边和测量线组成一个 `Scene` 交给 `Viewport::set_scene`，第 i 个球就是派生数据里第 i 个显示原子。视口在需要时渲染并异步回读，每完成一帧就通知自己；工作区观察视口，随之重绘测量标注等浮层，浮层位置由 `Viewport::project` 算出。

**生成超胞**（M5b）：对话框确认后，工作区在后台调用 `mw_crystal::make_supercell`，完成后发出 `StructureWorkspaceEvent::MadeSupercell`；外壳把结果作为派生结构加进来源所在的项目（`ProjectCatalog::add_derived`，记录矩阵和来源结构的哈希），起一个不重复的名字并打开。它像导入的结构一样先留在内存里，⌘S 时写进项目。

**编辑**（M4）：

1. 属性面板的字段按回车或离开时提交，元素选择器选中即提交；Delete、撤销、重做和"添加混合占位…"是动作。它们都交给 `StructureWorkspace::execute`，由它调用 `CrystalDocument::execute` 执行一条领域命令（`mw_crystal::Edit` 或测量的增删）。
2. 被拒绝时（如只删一个等效原子会破坏对称性），文档不变；界面按错误的引导弹对话框或通知。
3. 成功后得到 ID 映射和警告：选区和测量中途点选的原子按映射更新，警告变成通知，字段、场景和浮层重新同步。撤销和重做走同一条路。
4. 外壳为本次会话打开过的每个结构保留工作区（直到它的项目关闭），切换回来时编辑、撤销历史和视角都在；⌘S 把它们写进项目文件（M6）。

**点选与测量**：

1. 鼠标在视口上移动时，视口在后台读 ID 纹理，得到 `Pick::Sphere(i)` 或 `Pick::Bond(i)`，给悬停的原子画一圈浅色环，并发出 `ViewportEvent::Hovered`。
2. 按下后没有拖动就松开是一次点击：视口拾取后发出 `ViewportEvent::Clicked { pick, modifiers }`。
3. 工作区把球的下标换成 `AtomRef`。平时改选区：单击选中这个原子，⇧ 或 ⌘ 单击加选 / 取消，点空白处清空；测量模式下把原子加进正在进行的测量（见界面外壳设计稿 4.2 节）。
4. 工作区算出要标记的球（选区里的原子，测量模式下是已点的原子），交给 `Viewport::set_selection`；视口只上传高亮圈，不重建场景。

## 5. 命令与快捷键

- 每个 crate 用 `gpui_kit::actions!` 定义自己的动作，并在一个 `bind_keys` 函数里注册快捷键。外壳启动时依次调用。
- **按键上下文**：外壳根视图是 `Workbench`，结构工作区是 `StructureWorkspace`。只在工作区内有效的快捷键（如 Delete、M）绑定在 `StructureWorkspace` 上下文里；输入框有自己更深的上下文，所以打字不会触发这些快捷键。
- **动作处理器写在拥有该命令的视图上**（`.on_action(cx.listener(...))`）。快捷键、按钮、命令面板都派发同一个动作。
- **命令面板**（⌘K）由外壳打开，列出外壳命令，并在有打开的结构时加上 `mw_structure::palette_commands()`。确认后先关闭面板，把焦点还给打开前的位置，再派发动作，这样动作能沿焦点路径传到对应的视图。

新增一个命令的步骤见开发指南。

## 6. 国际化

- 使用 GPUI Kit 推荐的 `rust-i18n`。每个有界面文字的 crate 都有自己的 `locales/ui.yml`，同时写 `zh-CN` 和 `en`，回退到 `en`。
- 代码里用各 crate 定义的 `tr!("key", 参数 = 值)` 宏取文字，得到 `SharedString`。
- 默认界面语言是简体中文；设置对话框和命令面板都能切换，切换后刷新所有窗口。
- 用户数据（项目名、结构名、原子标签）不翻译。
- 导入诊断在 `mw-crystal-io` 里是数据（`DiagnosticKind`），由 `mw-structure/src/diagnostics.rs` 在渲染时翻译成当前语言，切换语言后已打开的诊断也会跟着变。
- 数字格式统一在 `mw-structure/src/format.rs`。

## 7. 主题与样式

- 颜色只从 `cx.theme()` 按语义取。唯一的例外是数据本身的颜色：元素颜色（Jmol 配色）在 `mw-structure/src/scene.rs`，坐标轴颜色以后也放在那里。
- 尺寸和间距用 rem 辅助函数。直接写像素只用于有注释说明的边界：初始窗口大小、标题栏按钮位置、测量标注相对锚点的偏移。
- 深色模式由 GPUI Kit 主题切换，视口背景跟随主题的 `muted` 色。

## 8. 3D 视口

技术方案和实测数据见[视口设计稿](design/viewport-rendering.md)。框架里的分工：

- `mw-crystal-render` 只负责"给定场景和相机，产出一帧像素"，以及"这个像素上是第几个球或键"。
- `mw-viewport` 负责相机交互（拖动旋转、滚轮缩放）、出帧和拾取流水线、悬停高亮和投影，不知道晶体是什么。
- `mw-structure` 决定画什么（原子颜色和半径、键、晶胞边、测量线、哪些原子标记为选中）以及上面叠什么（信息标签、悬停提示、测量提示与标注、浮动工具条）。

## 9. 模拟（Mocks）

所有模拟都在代码里用 `Mock:` 开头的注释标出（`rg "Mock:"` 可以全部列出）；用户能触发的模拟操作会弹出"尚未实现"的通知，而不是悄悄什么都不做。

| 模拟 | 位置 | 代替的是 | 由哪一步替换 |
|---|---|---|---|
| 显示固定为一个晶胞，面、棱、角上的原子在对面补一份 | `mw-crystal/src/expand.rs` 的 `display_atoms` | 视图层的显示范围 | v0.3 |
| 对称模式下移动位点：自由坐标可以输入，提交时提示尚未实现（P1 模式下是真实的） | `mw-structure/src/editing.rs` | 沿特殊位置移动与吸附（晶体学设计稿 7.4、7.5 节） | v0.2 |
| 显示范围、多面体、添加位点 | 各处的 `not_implemented` | 对应功能 | 见第 10 节；添加位点属于 v0.2 |
| P1 结构检测到更高对称性后的"对称化…"只提示尚未实现（检测本身是真实的） | `mw-structure/src/panel.rs` | `Symmetrize` 命令（晶体学设计稿 7.10 节） | v0.2 |
| "多面体"显示模型画成球棍 | `mw-structure/src/scene.rs` 的 `DisplayModel` | 半透明多面体管线 | v0.3 |
| MCP Server 打开的文件只留在进程里，放在一个名为 `session` 的项目下（资源 URI 里的项目一段） | `mw-mcp/src/session.rs` | 项目文件、`project.open` | v0.2：接上 M6 的项目文件，见 AI 集成设计稿 5.1 节 |

## 10. 路线图

完整的版本规划（v0.1 晶体学 → v0.4 衍射 → v0.5 热力学 → v0.6 显微组织 → v1.0）、每个版本的功能、算法与验收标准，见[产品路线图](roadmap.md)。

当前处于 v0.1，按以下里程碑把第 9 节的模拟逐个换成真实功能：

| 里程碑 | 内容 | 主要涉及 | 状态 |
|---|---|---|---|
| M1 CIF 导入 | CIF 1.1 子集解析、导入诊断（带行号）、"打开文件…"对话框和拖放；对称操作集（整数矩阵 + 有理平移、群封闭检查）和显示所需的最小原子展开 | 新增 `mw-crystal-io`；`mw-crystal` | 已实现。COD 语料统计（2026-10，不超过 1 MiB 的全部 504,373 个文件，占 COD 的 94 %；`crates/mw-crystal-io/tests/cod_corpus.rs`，开发指南第 2 节）：1,157 个文件（0.23 %）没有结构，其中 1,000 个条目本来就没有原子坐标，真正导入失败的 157 个（0.031 %），几乎都是只给超空间群的调制结构或没有可识别的对称性；导入器在任何文件上都不再 panic。修复前同一语料有 3,967 个文件（0.79 %）没有结构、81 个让导入器 panic，43.6 % 的文件误报多重度不一致（现在 0.22 %）。修复的问题见晶体学设计稿 9.1 节：SHELXL-97 把位点对称阶写在多重度字段里、重复的标签、占位率为 0 的行、行尾截断的 s.u.、位移参数里的笔误、没有坐标的占位行、带双撇号的 Hall 符号、新旧字段名只是写法不同。各类诊断的分布由测试打印 |
| M2 对称性 | 空间群识别、位点对称性与约束子空间、Wyckoff 指认，真正计算 `SymmetryAnalysis`；导入时吸附到特殊位置、合并混合占位 | `mw-crystal`（依赖 moyo，见晶体学设计稿 5.4、11 节）、`mw-crystal-io` | 已实现。验收：530 个表列设定（含 230 个标准设定）都能识别，其 3467 个 Wyckoff 位置的代表点都能指认回原字母；示例与原先手写的数据一致。导入时由空间群名称生成操作集、名称与操作集不符和晶胞与对称性不一致的警告也已加上；P1 结构会显示原子的对称性，对称化属于 v0.2。默认容差已用 COD 语料校准，维持 `position_tol` 0.01 Å、`symprec` 0.01 Å、`short_contact` 0.5 Å（晶体学设计稿 4.6 节）；晶胞与对称性不一致的警告阈值放宽到 1×10⁻³ |
| M3 渲染 | 键（impostor 圆柱）、晶胞边框、ID 纹理拾取；点选原子与测量模式 | `mw-crystal-render`、`mw-viewport` | 已实现。成键规则固定为默认值（共价半径之和 + 0.45 Å，金属之间不成键），还没有按元素对调整的设置；10 万原子 + 30 万键在全分辨率下约 30 fps（视口设计稿 8.2 节） |
| M4 基础编辑 | 命令与撤销栈：晶胞参数（按对称约束联动）、元素、占位率、删除位点；P1 模式下自由编辑；能力注册表 | `mw-crystal`、新增 `mw-automation`、`mw-structure` | 已实现。命令是可序列化的数据，每条都有精确的逆命令（晶体学设计稿 7.2、7.12 节）；对称模式下只改部分等效原子会被拒绝，引导为改整个位点或转为 P1（转为 P1 属于 M5b，届时接上）。能力注册表注册了 17 个结构查询与编辑能力，Schema 都在严格子集内（AI 集成设计稿第 4 节）。界面集成测试从这一步开始（`mw-structure/src/ui_tests.rs`）。对称模式下移动、添加位点仍属 v0.2 |
| M5 变换与导出 | 转为 P1、生成超胞、导出 CIF / POSCAR / 扩展 XYZ | `mw-crystal`、`mw-crystal-io` | 已实现：导出见晶体学设计稿 6、9.2–9.4 节，转为 P1 和生成超胞见下一行 M5b。CIF 写出对称操作、空间群名称、位点（含占位率、ADP 和不确定度）和派生字段，并写回源文件里模型不保存的字段；POSCAR（VASP 5，直接坐标）和扩展 XYZ 先展开为 P1，遇到部分占位或未知元素时拒绝并说明原因。导出对话框用系统保存面板写文件。往返测试覆盖全部测试文件；本地 COD 语料能导入的 503 216 个结构全部往返一致（2026-10-23，晶体学设计稿 9.2 节） |
| M5b 转为 P1 与超胞 | `ExpandToP1`（原结构上执行、可撤销）、`MakeSupercell`（新文档，记录来源） | `mw-crystal`、`mw-structure`、外壳 | 已实现（晶体学设计稿 7.1、7.9、7.11 节，项目文件设计稿第 13 节）。`ExpandToP1` 是编辑命令，逆命令恢复整个结构；ID 映射新增拆分与合并，选区和测量跟着原子转到 P1 位点，撤销后回来；删除对话框的"转为 P1 后删除"是一步。`make_supercell` 接受任意正行列式的整数矩阵，界面给对角矩阵；ADP 与不确定度随操作和换基变换。超胞在后台生成，作为派生结构排在来源之后、记录矩阵和来源结构的哈希。示例项目里的 2×2×2 超胞改由它生成，与原先手写的逐位相同。转为 P1 和生成超胞的能力（AI 集成设计稿 5.2 节）留到 v0.3 |
| M6 持久化 | 项目文件、最近项目、设置保存；示例结构改为从项目文件读 | 新增 `mw-project-file`；`mw-project`、`mw-structure`、外壳 | 已实现（[项目文件设计稿](design/project-format.md)）。项目是 `.mwproj` 目录：清单、每个结构一个文件（结构、测量、编辑记录）、导入文件的原样副本（按 SHA-256 命名，导入诊断和 CIF 保留字段由它重新解析）、视图状态；格式版本 1，旧格式在内存里迁移，第一次保存前备份。新建、打开、保存、另存为、关闭项目，未保存时先问；最近项目、退出时打开着的项目和设置存在系统配置目录里。示例项目由 fixture 真实导入生成，随应用附带，第一次启动装到数据目录。验收：全部示例和每个 fixture 结构写成项目再读回逐项相同；界面集成测试编辑、测量、换视角后 ⌘S、关闭、再打开，状态完整恢复（`apps/workbench/src/ui_tests.rs`） |

对称模式下移动、添加原子（特殊位置吸附）、对称性检测与对称化、晶胞变换属于 v0.2。

v0.2 的实验性 MCP Server 已提前做出原型（`mw-mcp`，2026-10-22）：无界面、只读、stdio，独立可执行文件；能力注册表为此加了 `structure.diagnostics` 和 `structure.export`。连接运行中的应用、项目工具（等 M6）和并入 `materials-workbench mcp` 见 AI 集成设计稿 5.1 节。
