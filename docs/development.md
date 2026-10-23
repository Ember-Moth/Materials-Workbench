# 开发指南

> 架构与模块划分见 [架构](architecture.md)。本文讲怎么构建、怎么写代码、怎么交付一个功能。

## 1. 环境

| 平台 | 需要 |
|---|---|
| 全部 | Rust 1.92 或更新（GPUI Kit 当前依赖要求）；本仓库在 1.97 上开发 |
| macOS | Xcode（GPUI 用 Metal），`xcode-select -p` 指向 Xcode |
| Linux | 图形会话（Wayland 或 X11）和可用的 Vulkan 驱动；依赖包见 [GPUI Kit 安装说明](https://gpui-kit.com/docs/installation) |
| Windows | 尚未测试 |

## 2. 常用命令

```bash
cargo run --bin materials-workbench
```

```bash
cargo test --workspace
```

```bash
cargo clippy --workspace --all-targets
```

```bash
cargo fmt --all
```

CIF 导入的语料统计：读取本地一个 COD 目录（不随仓库提交）。语料从 COD 的官方 rsync 服务器取，数据属于公有领域；`--max-size=1M` 只取不超过 1 MiB 的文件（约 50 万个、37 GB，占全部条目的 94 %，超过的大多嵌着衍射数据），按前缀目录 1–9 分别同步，以后重跑同一条命令只补新增和改动的条目：

```bash
rsync -rt --max-size=1M rsync://www.crystallography.net/cif/7/ ~/cod/cif/7/
```

测试默认忽略，用 release 构建跑得快得多，文件分到所有核上读：

```bash
MW_COD_DIR=~/cod/cif cargo test -p mw-crystal-io --release --test cod_corpus -- --ignored --nocapture
```

| 测试 | 内容 |
|---|---|
| `cod_corpus_statistics` | 没有结构的文件数（区分"条目本来没有坐标"和"导入失败"）、每种诊断的次数和文件数、常见细节、示例文件、最慢的文件 |
| `cod_position_tolerance` | 用几档 `position` 容差各导入一遍，比较吸附、合并、多重度不一致等诊断的次数（晶体学设计稿 4.6 节），全量约 40 分钟 |
| `cod_short_contacts` | 满占位有序位点之间 1 Å 以内的距离分布，分含氢和不含氢 |
| `cod_symprec` | 把抽样的结构四舍五入成 P1 文件的样子，看对称性搜索在几档 `symprec` 下能否找回原来的操作集 |
| `cod_file` | 打印 `MW_COD_FILE` 指定文件（用 `:` 分隔）的全部诊断和行号，用来查看单个条目 |

环境变量：`MW_COD_LIMIT=1000` 只读前 1000 个文件；`MW_COD_LIST=atom.duplicate_label` 额外列出带这个诊断的文件（最多 100 个）；`MW_COD_SAMPLE=20` 是 `cod_symprec` 的抽样间隔。按名字只跑其中一个，例如 `... --test cod_corpus cod_symprec -- --ignored --nocapture`。

CIF 往返的语料统计：对同一个目录里每个能导入的结构做"导入 → 导出 → 再导入"，打印往返一致的比例和各类不一致的数量与例子（比较规则见晶体学设计稿 9.2 节）。同样默认忽略，`MW_COD_LIMIT` 同样适用：

```bash
MW_COD_DIR=~/cod/cif cargo test -p mw-crystal-io --release --test cod_roundtrip -- --ignored --nocapture
```

随应用附带的示例项目（`crates/mw-project-file/samples/`）由测试从 `mw-crystal-io` 的 fixture 生成，改了项目格式或这些 fixture 后重新生成（普通测试会检查它们是不是最新的）：

```bash
cargo test -p mw-project-file --test samples -- --ignored
```

不开窗口、只看渲染器输出：

```bash
cargo run -p mw-crystal-render --example snapshot -- snapshot.png 1000
```

视口性能基准（会打开窗口，约 1 分钟后自动退出；先测只有原子的各档，再测带键的几档，每档之后计时拾取延迟。单测某几档的写法见[视口设计稿](design/viewport-rendering.md) 8.2 节）：

```bash
VIEWPORT_POC_BENCH=1 cargo run -p viewport-poc
```

界面截图巡览：依次切换主界面、位点面板、在视口里点选原子（先拖动旋转，再单击、⇧ 单击、悬停，用的是真实的鼠标事件和拾取）、测量距离和键角、命令面板、删除对话框、深色模式、英文界面、其他示例结构，在 SrTiO₃ 上编辑（改晶胞参数、添加混合占位的对话框和结果、点选并删除位点、撤销回原样），在 SrTiO₃ 上转为 P1 后删除一个原子，保存项目（⌘S），再导入 `crates/mw-crystal-io/tests/fixtures/` 里的几个文件（带诊断的、吸附到特殊位置的、导入失败的及其错误列表），金红石的转为 P1 对话框和结果、生成超胞的对话框和新的派生结构，关闭有未保存更改的项目时的提示，最后是设置对话框、收起边栏、导出对话框（部分占位的结构，POSCAR 和 XYZ 置灰）、带最近项目的空状态和新建的空项目，把每一步渲染成 PNG 后自动退出。每步之后等视口的最新一帧画出来（`Viewport::is_current`）再截图。巡览用 `screenshots/config` 作配置目录，每次都从第一次启动开始（装好示例项目），不读也不改你自己的设置、最近项目和示例项目。它用 GPUI 的离屏渲染，不需要屏幕录制权限，适合改完界面后自查：

```bash
MW_SCREENSHOT_DIR=screenshots cargo run --bin materials-workbench --features screenshot
```

系统不重绘被遮挡、在后台或显示器已休眠时的窗口，所以巡览每一步都自己画一遍窗口（`Window::draw`）再截图，窗口不必在前台，你可以照常用电脑。如果某张和上一张完全相同，或整张空白，说明那一步没有生效，看日志里的警告。

内置助手的模型服务客户端（`mw-llm`）可以单独试：示例程序发一轮对话，把流式事件打印出来，文本到标准输出，推理摘要、工具调用的校验结果和用量到标准错误。它会向模型服务发真实请求并按用量计费（本地模型除外）。配置全部来自环境变量，密钥不要写进命令行参数，也不会出现在日志里：

| 变量 | 含义 |
|---|---|
| `MW_LLM_PROVIDER` | 厂商档案的 id：`anthropic`（默认）、`openai`（Responses）、`openai-chat`、`deepseek`、`dashscope`、`moonshot`、`zhipu`、`siliconflow`、`openrouter`、`ollama`、`vllm`、`lm-studio`、`azure-openai`；自定义端点写 `custom:openai_chat`、`custom:openai_responses` 或 `custom:anthropic`，按最保守的能力处理 |
| `MW_LLM_BASE_URL` | 覆盖档案里的 base URL；自定义端点和 Azure 必填 |
| `MW_LLM_MODEL` | 模型名；`anthropic` 默认 `claude-opus-5-5`，其他必填 |
| `MW_LLM_API_KEY` | 密钥；本地模型不用设 |
| `MW_LLM_EFFORT` | `low`、`medium`、`high`、`xhigh`、`max` |
| `MW_LLM_TOOLS` | 设为 `1` 时提供一个演示工具，用来看工具调用的流式参数和本地校验 |

在当前 shell 里设好 `MW_LLM_API_KEY` 后，问 Claude 一个问题（参数是提问内容）：

```bash
cargo run -p mw-llm --example chat -- "什么是 Wyckoff 位置？"
```

用本机的 Ollama 试 Chat Completions 后端，不花钱（模型名换成本机已拉取的模型）：

```bash
MW_LLM_PROVIDER=ollama MW_LLM_MODEL=qwen3 MW_LLM_TOOLS=1 cargo run -p mw-llm --example chat
```

加上 `RUST_LOG=mw_llm=debug` 可以看到请求地址和重试；日志里没有请求内容和密钥。

debug 构建里，GPUI 和 wgpu 已按 GPUI Kit 的建议以 `opt-level = 3` 编译（见根目录 `Cargo.toml`），所以界面在 debug 下也足够流畅。

## 3. 代码约定

### 3.1 必读

界面代码必须遵守 GPUI Kit 的两份规范：Coding Guides 和 Design Guides（[gpui-kit.com/docs](https://gpui-kit.com/docs)）。以下是本项目里最常用到的几条，不能代替原文：

- **不要凭印象写 API。** 先在 `~/.cargo/registry/src/*/gpui-pre-*`、`gpui-component-*`、`gpui-base-*`、`wgpu-*` 里找到真实签名。版本以 `Cargo.lock` 为准（当前 GPUI Kit 0.8.0、`gpui-pre` 0.3.9、wgpu 29）。
- **优先用标准组件。** 菜单、选择器、命令面板、侧边栏、列表行都有现成组件，不要用 `div` 拼一个"看起来像"的。
- **状态放在能保证它正确的最窄的地方。** 跨帧的行为用 `Entity<T>`，纯展示用 `RenderOnce`。`render` 里不改状态、不无条件 `notify`。
- **重复元素用领域 ID 作 `ElementId`**，不用列表下标。

### 3.2 放在哪里

- 一个新能力（例如 XRD）是一个新 crate，包含它的模型、视图、命令和对话框。新 crate 的 `[package]` 里写 `edition.workspace = true`、`publish.workspace = true` 和 `license.workspace = true`，从工作区继承版本、发布设置和许可证。
- 新增依赖前确认它的许可证与 MIT OR Apache-2.0 兼容（MIT、Apache-2.0、BSD、ISC、Zlib 等宽松许可）；不引入 GPL 类许可的依赖，也不复制 GPL 代码。
- 纯计算（解析、对称性、物理量）放在不依赖 GPUI 的 crate 里，并在那里写单元测试。
- `apps/workbench` 只做组装：创建实体、转接事件、注册全局命令，不放业务逻辑。

### 3.3 样式

- 颜色只用 `cx.theme()` 的语义色。数据颜色（元素颜色等）是唯一例外，集中放在 `mw-structure/src/scene.rs` 并加注释。
- 尺寸用 rem 辅助函数（`p_2()`、`w_80()`、`text_sm()`）。必须写像素时要有注释说明原因（平台边界、测量出的几何）。

### 3.4 图标

- GPUI Kit 组件自带的 `IconName` 只是 Lucide 的一个子集。需要其他图标时用 `gpui_kit::assets::IconName`（完整集合），包成 `Icon::new(...)` 传给组件。
- 完整集合里的图标要由可执行程序打包：用到子集之外的图标时，把它加进 `apps/workbench/src/assets.rs` 的列表，否则运行时会报"找不到资源"，图标显示为空白。

### 3.5 文字

- 所有界面文字写进所在 crate 的 `locales/ui.yml`，同时提供 `zh-CN` 和 `en`，代码里用 `tr!` 取。
- 每种语言按自己的习惯写，不逐字对译；完整的句子放在一个键里，用 `%{name}` 传参数，不要拼接翻译片段。
- 打开对话框或需要进一步输入的按钮、菜单项结尾加省略号 `…`（单个字符）。
- 术语遵循[界面外壳设计稿](design/ui-shell.md) 第 7 节的术语表。
- 领域 crate 不放界面文字。比如导入诊断在 `mw-crystal-io` 里是数据（`DiagnosticKind`），文字在 `mw-structure` 的 `locales/ui.yml` 里，新增一种诊断要同时加上两种语言的文字。

### 3.6 命令

新增一个命令：

1. 在拥有它的 crate 里用 `gpui_kit::actions!` 定义动作。
2. 在该视图上用 `.on_action(cx.listener(...))` 处理。
3. 需要快捷键时，在该 crate 的 `bind_keys` 里注册，并选对按键上下文。
4. 加进命令面板：结构相关的加到 `mw_structure::palette_commands()`，外壳命令加到 `apps/workbench/src/palette.rs`。关键词里写英文名和拼音。
5. 按钮、菜单都派发同一个动作，不要各自复制一份逻辑。

### 3.7 模拟

- 模拟代码用 `Mock:` 开头的注释标出，说明它代替的是什么。
- 用户能触发的未实现操作必须给出反馈：调用 `not_implemented` 弹出"尚未实现"通知。
- 替换掉一个模拟后，同步更新[架构](architecture.md)第 9 节的模拟表。

## 4. 测试

| 层次 | 例子 | 位置 |
|---|---|---|
| 纯单元测试 | 晶胞矩阵与体积、对称操作与群封闭、原子展开、转为 P1 与超胞（位移参数和不确定度的变换、与手写超胞逐位相同、非对角矩阵）、530 个表列设定、空间群识别、位点对称性与坐标形式、晶胞约束、成键、测量、化学式、近邻、编辑命令（每条命令和它的逆命令精确往返）、撤销与重做、选区、CIF 语法、数字格式 | 各 crate 的 `#[cfg(test)]` |
| 导入测试 | 每个 fixture CIF 导入后的结构和诊断（含行号）、空间群号与 Wyckoff 位置、吸附与合并、P1 结构的对称性检测 | `crates/mw-crystal-io/tests/fixtures.rs` |
| 导出测试 | 每个 fixture CIF 的"导入 → 导出 → 再导入"往返（结构、保留字段、二次导出文本相同）；保留字段在结构修改后的去留；其他设定的空间群名称；POSCAR 与扩展 XYZ 对照手写的期望文件；部分占位和未知元素的拒绝 | `crates/mw-crystal-io/tests/export.rs` |
| 项目文件 | 全部内置示例和每个 fixture 能导入的结构写成项目再读回，结构、测量（含已失效的）、编辑记录、来源记录、视图状态逐项相同，重写一遍字节相同；只重写改过的文件、另存为、清理不再引用的文件、读不了的部分（损坏、版本更新、源文件副本被改或缺失）；示例项目与重新生成的一致 | `crates/mw-project-file/tests/roundtrip.rs`、`samples.rs` |
| 语料统计 | COD 批量导入与 CIF 往返，默认忽略，见第 2 节 | `crates/mw-crystal-io/tests/cod_corpus.rs`、`cod_roundtrip.rs` |
| 无头 GPU 测试 | 渲染一帧并检查像素（球、键的两端颜色、遮挡），以及拾取结果 | `mw-crystal-render` |
| 模型服务的流式解析 | 按官方文档格式手写的 SSE 数据，逐个后端检查事件、组装出的轮次、拒绝和中途错误；请求 JSON 的构造 | `crates/mw-llm/tests/fixtures/` 与各后端模块的测试 |
| 模型服务的 HTTP 路径 | 本机回环上的临时服务器：请求头与鉴权、错误状态、丢弃流即断开连接。不访问外网 | `crates/mw-llm/tests/provider.rs` |
| 能力注册表 | 每个能力的输入、输出 Schema 都在严格子集内（`mw_llm::schema::check_schema`），全部能力放进一个严格请求也合规（`check_tools`）；在内存里的文档上逐个调用，输入和输出都按 Schema 校验，并检查拒绝时的错误码和引导 | `crates/mw-automation/tests/capabilities.rs` |
| 外壳的界面集成测试 | 在无头窗口里用临时配置目录启动外壳：第一次启动打开示例项目；编辑、测量、换视角后 ⌘S、关闭项目、再打开，结构、测量、编辑记录和视图状态完整恢复（路线图 M6 的验收）；有未保存的更改时关闭和退出先问；导入的文件随项目保存，带源文件副本；生成的超胞作为派生结构排在来源之后，保存后带着矩阵和来源记录 | `apps/workbench/src/ui_tests.rs` |
| 界面集成测试 | 用 `#[gpui_kit::test]` 在无头窗口里打开工作区，像用户一样聚焦字段、打字、按键（回车、Delete、Esc、⌘Z / ⇧⌘Z），检查字段显示的值和文档的变化：晶胞参数联动与撤销重做、被拒绝的输入恢复原值、混合占位的字段、元素选择器、删除位点与对称模式下的对话框、转为 P1（选区跟随、撤销一步回到原样）与"转为 P1 后删除"、生成超胞交给外壳、坐标编辑。GPUI Kit 的 `test-support` 与本工作区的依赖冲突，所以直接开启 `gpui-pre` 的 `test-support`，用 GPUI 的 `VisualTestContext` 驱动，没有 `gpui_kit::test` 的按 ID 点击 | `crates/mw-structure/src/ui_tests.rs` |
| 截图巡览 | 用 `screenshot` 特性把主要界面渲染成 PNG 逐张检查 | `apps/workbench/src/screenshot.rs` |
| 手动检查 | 在窗口里走一遍改动涉及的流程，包括深色模式和英文界面 | — |

改动界面时，至少手动检查：键盘能否完成操作、焦点去向、深浅色主题、中英文两种语言下的文字长度。

### 4.1 测试数据

- 测试用的结构文件放在使用它的 crate 的 `tests/fixtures/` 里，每个文件开头注释写明内容和来源；目录里的 `README.md` 列出所有文件的来源。
- 优先手写小文件。取自 COD 等数据库的文件，先核实许可，并在文件开头注明条目号和许可。
- 完整的数据库语料不提交，用环境变量指向本地目录。
- 模型服务的测试不发真实请求：真实 API 需要密钥，而且收费。流式数据按官方文档手写，来源记在 `crates/mw-llm/tests/fixtures/README.md`；要对真实服务验证，用上面第 2 节的示例程序手动跑。

## 5. 交付一个功能

把一个模拟换成真实功能时，按这个顺序：

1. 读对应设计稿里的相关章节；设计需要改动时，先改设计稿。
2. 在不依赖界面的 crate 里实现逻辑，并写单元测试。
3. 接到界面上，替换掉模拟，删除对应的 `not_implemented` 调用。
4. 更新架构文档的模拟表和路线图。
5. 运行 `cargo fmt --all`、`cargo clippy --workspace --all-targets`、`cargo test --workspace`，再在应用里手动走一遍。

## 6. 提交

- 使用 Conventional Commits：`feat:`、`fix:`、`docs:`、`refactor:`、`test:`、`chore:`。
- 标题用英文、祈使语气；正文说明改了什么、为什么，需要时列出要点。
- 一次提交只做一件事；文档跟随它描述的代码一起提交。

## 7. 文档

- 文档用中文写，API 名、crate 名、文件名保留原文并用代码格式。
- 模块设计放在 `docs/design/`。实现与设计不一致时，以修订后的设计稿为准，不要让两者长期分叉。
