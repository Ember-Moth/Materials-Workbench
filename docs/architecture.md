# 架构

> 状态：v0.1 框架已搭好，M1 CIF 导入、M2 对称性、M3 渲染（键、晶胞边框、拾取、测量）已实现，其余功能以模拟（mock）为主 · 2026-10-22
> 相关设计：[界面外壳](design/ui-shell.md) · [晶体学数据模型](design/crystallography-model.md) · [3D 视口](design/viewport-rendering.md) · [AI 集成](design/ai-integration.md)
> 开发流程与约定见 [开发指南](development.md)；版本规划见 [产品路线图](roadmap.md)。

## 1. 目标

Materials Workbench 是一个用 Rust 编写的现代化、开源、跨平台的材料科学桌面工作台。第一阶段做晶体学：查看、测量、编辑晶体结构，再导出给下游计算。之后逐步加入 XRD、相图和显微组织分析。

当前状态：界面按设计稿实现；可以打开 CIF 文件（M1），对称操作集、原子展开、成键和对称性分析（空间群识别、位点对称性与坐标约束、Wyckoff 位置、晶胞约束，导入时吸附到特殊位置并合并混合占位，M2）是真实计算的；视口画出原子、键和晶胞边框，可以点选原子、测量距离和键角（M3）。编辑、导出和项目文件仍是模拟。第 9 节列出所有模拟，第 10 节给出替换它们的顺序。

架构上为后续模块预留的位置：XRD、热力学、显微组织各自是"计算 crate（无界面）+ 界面能力 crate"的组合，与晶体学模块并列，共享项目系统、命令与撤销、视口与图表、国际化。计划中的 crate 见[产品路线图](roadmap.md)第 8–10 节。

## 2. 仓库结构

```text
materials-workbench/
├── apps/
│   ├── workbench/            桌面应用（可执行文件 materials-workbench）
│   └── viewport-poc/         视口基准测试工具
├── crates/
│   ├── mw-crystal/           晶体结构模型（纯 Rust，无界面）
│   ├── mw-crystal-io/        结构文件读写（纯 Rust，无界面）：目前是 CIF 导入
│   ├── mw-crystal-render/    wgpu 离屏渲染器（不依赖 GPUI）
│   ├── mw-viewport/          3D 视口组件（GPUI + 渲染器）
│   ├── mw-project/           项目：项目目录与左侧边栏
│   ├── mw-structure/         结构工作区：文档、视图、面板、对话框、命令
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

`mw-project` 和 `mw-structure` 也直接使用 `mw-crystal` 的类型，图中省略了这两条边。`mw-llm` 不依赖其他 crate，目前也没有使用者；以后由内置助手 `mw-assistant` 依赖（[AI 集成设计稿](design/ai-integration.md)第 3 节）。

| crate | 职责 | 依赖 GPUI |
|---|---|---|
| `mw-crystal` | 晶体结构的数据类型（晶胞、对称操作集、位点、容差）、原子展开与派生数据（含 `AtomRef` 和成键）、测量的几何、对称性分析（空间群识别、位点对称性、Wyckoff 位置、晶胞约束；符号表和对称性搜索来自 moyo，只在 `tables.rs` 里调用）、元素表，以及内置示例 | 否 |
| `mw-crystal-io` | 解析结构文件（CIF 1.1 子集），整理位点（吸附到特殊位置、合并混合占位、去掉重复行），产出结构和带源文件行号的导入诊断；诊断是数据，文字由界面翻译 | 否 |
| `mw-crystal-render` | 离屏渲染球、键、线段和高亮圈并回读像素；按需渲染 ID 纹理回答"鼠标下是什么"；只依赖 wgpu | 否 |
| `mw-viewport` | 把渲染结果放进 GPUI：相机交互、异步出帧与拾取、悬停高亮、3D 坐标投影到屏幕 | 是 |
| `mw-project` | 项目目录模型和左侧边栏视图 | 是 |
| `mw-structure` | 一个打开的结构：顶栏、视口与浮层、右侧面板、对话框、命令；选区与测量；打开文件后的通知与错误列表 | 是 |
| `apps/workbench` | 窗口外壳：组装边栏与工作区、全局命令、命令面板、主题与语言 | 是 |
| `mw-llm` | 内置助手的模型服务客户端：`ChatProvider` 接口和项目自己的对话类型；Anthropic Messages、OpenAI Chat Completions（含兼容服务和本地模型）、OpenAI Responses 三个后端；工具 Schema 子集检查与参数校验；能力声明与厂商档案。HTTP 跑在专用线程的 tokio runtime 上，流可以由任何执行器轮询 | 否 |

**规则**：

- **按能力分 crate，而不是按"视图 / 模型 / 对话框"分目录**（GPUI Kit Coding Guides）。一个能力的模型、视图、命令和对话框放在同一个 crate 里。
- **领域 crate 不依赖界面**。`mw-crystal`（含对称性分析）和 `mw-crystal-io` 都要能在 CLI 或测试里单独使用。
- **渲染器不依赖 GPUI**，见视口设计稿 5.1 节。
- **能力之间不互相依赖**。`mw-project` 和 `mw-structure` 互不引用，由外壳通过事件转接（见第 4 节）。
- 只有一个能力在用的东西不提前抽成共享 crate；有第二个使用者时再抽。

## 4. 运行时结构

```text
Workbench（窗口根视图，apps/workbench）
├── ProjectSidebar（mw-project）
├── StructureWorkspace（mw-structure），没有打开结构时为空状态
│   ├── StructureDocument：结构 + 派生数据 + 测量 + 诊断
│   ├── Selection：位点列表和视口共用的选区
│   ├── Viewport（mw-viewport）：渲染器、相机、出帧与拾取流水线、悬停高亮
│   └── 各字段的 InputState
└── CommandState：命令面板的查询与选择状态
```

**打开结构的流程**：

1. 用户点击边栏里的结构，`ProjectSidebar` 发出 `ProjectSidebarEvent::OpenStructure(id)`。
2. `Workbench` 在项目目录里查到这个结构，按它的 `StructureSource` 构造 `StructureDocument`：内置示例，或本次会话导入的数据块。
3. `Workbench` 新建一个 `StructureWorkspace`，把焦点交给它，并告诉边栏选中项变了。

**打开文件的流程**：

1. 边栏、⌘O、命令面板和空状态里的"打开文件…"都派发 `OpenFile`；`Workbench` 用 `prompt_for_paths` 弹出系统文件对话框。拖进窗口的文件由根视图的 `on_drop::<ExternalPaths>` 接收，走同一条路。
2. 后台任务里，`mw_structure::import::OpenedFile::read` 读文件，用 `mw_crystal_io::cif::read` 解析。
3. 回到界面线程，每个成功的数据块加入项目目录里的"导入的文件"项目（`ProjectCatalog::add_imported`），然后打开第一个。
4. `OpenedFile::notify` 告诉用户结果：顺利导入时不提示（打开的结构本身就是反馈）；有诊断时发通知，诊断在"诊断"标签页里；导入失败时发错误通知，通知上的"查看错误…"打开带行号的错误列表。

**视口出帧**：`StructureWorkspace` 把文档的显示原子、显示键、晶胞边和测量线组成一个 `Scene` 交给 `Viewport::set_scene`，第 i 个球就是派生数据里第 i 个显示原子。视口在需要时渲染并异步回读，每完成一帧就通知自己；工作区观察视口，随之重绘测量标注等浮层，浮层位置由 `Viewport::project` 算出。

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
| 内置示例结构（金红石、2×2×2 超胞、SrTiO₃）；操作集、展开和对称性分析是真实的 | `mw-crystal/src/sample.rs` | 项目文件里保存的结构 | M6 项目与设置持久化 |
| 项目目录；打开的文件只留在内存里，统一放进"导入的文件"项目 | `mw-project` 的 `ProjectCatalog` | 项目文件、新建项目 | M6 项目与设置持久化 |
| 超胞的诊断 | `mw-structure/src/document.rs` | 派生结构的来源记录 | M5 |
| 显示固定为一个晶胞，面、棱、角上的原子在对面补一份 | `mw-crystal/src/expand.rs` 的 `display_atoms` | 视图层的显示范围 | v0.3 |
| 导入时不保留未识别的字段 | `mw-crystal-io` | CIF 往返导出 | M5 变换与导出 |
| 字段可以输入，但不会改变结构 | `mw-structure` 的属性面板 | 编辑命令与撤销 | M4 编辑 |
| 删除、转为 P1、生成超胞、导出：对话框齐全，确认后只提示"尚未实现" | `mw-structure/src/dialogs.rs` | 对应的编辑命令 | M4、M5 |
| 新建项目、显示范围、多面体、更换元素、混合占位 | 各处的 `not_implemented` | 对应功能 | 见第 10 节 |
| P1 结构检测到更高对称性后的"对称化…"只提示尚未实现（检测本身是真实的） | `mw-structure/src/panel.rs` | `Symmetrize` 命令（晶体学设计稿 7.10 节） | v0.2 |
| "多面体"显示模型画成球棍 | `mw-structure/src/scene.rs` 的 `DisplayModel` | 半透明多面体管线 | v0.3 |
| 设置不保存 | `apps/workbench/src/settings.rs` | 设置持久化 | M6 |

## 10. 路线图

完整的版本规划（v0.1 晶体学 → v0.4 衍射 → v0.5 热力学 → v0.6 显微组织 → v1.0）、每个版本的功能、算法与验收标准，见[产品路线图](roadmap.md)。

当前处于 v0.1，按以下里程碑把第 9 节的模拟逐个换成真实功能：

| 里程碑 | 内容 | 主要涉及 | 状态 |
|---|---|---|---|
| M1 CIF 导入 | CIF 1.1 子集解析、导入诊断（带行号）、"打开文件…"对话框和拖放；对称操作集（整数矩阵 + 有理平移、群封闭检查）和显示所需的最小原子展开 | 新增 `mw-crystal-io`；`mw-crystal` | 已实现。COD 语料的批量统计（`crates/mw-crystal-io/tests/cod_corpus.rs`）还没有在完整语料上跑过 |
| M2 对称性 | 空间群识别、位点对称性与约束子空间、Wyckoff 指认，真正计算 `SymmetryAnalysis`；导入时吸附到特殊位置、合并混合占位 | `mw-crystal`（依赖 moyo，见晶体学设计稿 5.4、11 节）、`mw-crystal-io` | 已实现。验收：530 个表列设定（含 230 个标准设定）都能识别，其 3467 个 Wyckoff 位置的代表点都能指认回原字母；示例与原先手写的数据一致。导入时由空间群名称生成操作集、名称与操作集不符和晶胞与对称性不一致的警告也已加上；P1 结构会显示原子的对称性，对称化属于 v0.2。默认容差仍待 COD 语料统计后校准 |
| M3 渲染 | 键（impostor 圆柱）、晶胞边框、ID 纹理拾取；点选原子与测量模式 | `mw-crystal-render`、`mw-viewport` | 已实现。成键规则固定为默认值（共价半径之和 + 0.45 Å，金属之间不成键），还没有按元素对调整的设置；10 万原子 + 30 万键在全分辨率下约 30 fps（视口设计稿 8.2 节） |
| M4 基础编辑 | 命令与撤销栈：晶胞参数（按对称约束联动）、元素、占位率、删除位点；P1 模式下自由编辑 | `mw-crystal`、`mw-structure` | 未开始 |
| M5 变换与导出 | 转为 P1、生成超胞、导出 CIF / POSCAR / 扩展 XYZ | `mw-crystal`、`mw-crystal-io` | 未开始 |
| M6 持久化 | 项目文件、最近项目、设置保存；示例结构改为从项目文件读 | `mw-project`、外壳 | 未开始 |

对称模式下移动、添加原子（特殊位置吸附）、对称性检测与对称化、晶胞变换属于 v0.2。
