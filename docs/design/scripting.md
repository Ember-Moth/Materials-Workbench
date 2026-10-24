# 命令行与 Python

> 状态：M19 已按本稿实现（2026-10-24）；第 9 节的三个决定待你确认
> 范围：命令行 `mw`（`mw-cli`）、Python 模块 `materials_workbench`（`mw-python`）和两者共用的 `mw-scripting`；它们在 CI 里的构建。
> 相关：[产品路线图](../roadmap.md)第 7、14、15 节 · [AI 集成](ai-integration.md)第 4 节（能力注册表）· [晶体学数据模型](crystallography-model.md) 7.8–7.10 节（晶胞变换、超胞、对称化）、第 9 节（文件格式）· [界面外壳](ui-shell.md)第 3 节（按文件名认格式）· [开发指南](../development.md) 2.1 节（CI）

## 1. 目标

在服务器、CI 和 Jupyter 里不开窗口就能做界面里做的事：格式转换、超胞、晶胞变换、对称性检测与对称化、结构摘要、导入诊断，一次处理一批文件。验收（[架构](../architecture.md)第 10 节）：命令行和 Python 都能完成格式转换、超胞和对称性检测，结果与界面一致。

## 2. 一套能力，多个入口

路线图第 14 节的原则：GUI、命令行、Python、MCP 调用的是同一批领域命令和查询。命令行和 Python 不各写一套，而是共用 `mw-scripting`：

- **能力注册表里有的，就调注册表**。每个结构放在一个只装它自己的 `mw_automation::Documents` 宿主里，用 `Registry::standard()` 按名字调用，和 MCP Server、连接应用时走的是同一个处理函数，输出是同一份 JSON。
- **注册表还没有的，调界面调用的同一个领域函数**，选项也取界面的。
- **读文件按界面的规则**：认格式（`.cif` 是 CIF；`.vasp`、`.poscar` 和以 POSCAR、CONTCAR 开头的文件名是 POSCAR，界面外壳设计稿第 3 节）、给结构起名（文件名去掉扩展名，多个数据块时加 ` · 数据块名`，只叫 POSCAR 或 CONTCAR 的加文件夹名）、把文件级的诊断放到每个结构的诊断前面，都与 `mw_structure::import::OpenedFile` 一致。

| 操作 | 命令行 / Python 调用的 | 界面调用的 |
|---|---|---|
| 结构摘要 | `structure.summary` | 属性面板读同一份 `Derived` |
| 位点列表 | `structure.sites`（逐页取全） | 位点面板 |
| 导入诊断 | 结构：`structure.diagnostics`；整个文件（含导入失败的数据块）：同样的字段，英文说明来自 `mw_automation::diagnostic_message` | "诊断"标签页（本地化文案） |
| 对称性检测 | `structure.detect_symmetry` | "对称化…"对话框的预览（`DetectSymmetry`） |
| 对称化 | `structure.symmetrize` | 同一条 `Edit::Symmetrize` |
| 晶胞变换 | `structure.transform_cell` | 同一条 `Edit::TransformCell` |
| 撤销、重做 | `history.undo`、`history.redo` | `CrystalDocument::undo` / `redo` |
| 超胞 | `mw_crystal::make_supercell`（注册表还没有 `structure.make_supercell`，M18 加） | 同一个函数，名字同为"`名称` a×b×c supercell" |
| 写出文件文本 | `cif::write`（导入的结构用源数据块的 `write_options`）、`poscar::write`、`xyz::write`，与 `structure.export` 相同的选择和错误码，但不限 1 MiB | 导出对话框的同一组函数 |
| 任意能力 | 按名字调用注册表 | — |

POSCAR 的注释行：界面写"结构名 · 项目名"，`structure.export` 写"结构名 · 宿主给的项目名"；命令行和 Python 没有项目，宿主的项目名取来源文件名，所以写"结构名 · 来源文件名"（如 `rutile · rutile.cif`），从标准输入或文本读入的写结构名。导入的结构写 CIF 时，数据块名和保留字段来自源数据块，与界面相同；其余结构的数据块名用结构名（界面用文件名，`structure.export` 用结构名）。

"一致"的含义：同一个文件得到同样的结构（逐位相同）、同样的注册表输出、同样的导出文本（除上面说的注释行和数据块名）。测试按这个标准逐项比较（第 7 节）。

## 3. crate 划分

```text
mw-cli（可执行文件 mw）     mw-python（扩展模块 materials_workbench）
          └──────────────┬──────────────┘
                         ▼
                   mw-scripting
                         │
          ┌──────────────┼───────────────┐
          ▼              ▼               ▼
    mw-automation   mw-crystal-io    mw-crystal
```

- **`mw-scripting`**（纯 Rust）：`Format`（`cif`、`poscar`、`xyz`，`of_input` / `of_output` 按文件名认格式）、`read_file` / `read_bytes`（得到 `FileReport`：导入报告和起好名的结构）、`Document`（一个结构：宿主、名称、来源文件名；查询和编辑经注册表，超胞和写出经领域函数）、`NewCell`（晶胞变换的五种选择）、`FileReport::diagnostics_json` 和 `capabilities_json`（命令行和 Python 输出的同一份 JSON）。两个使用者从第一天就有，符合"有第二个使用者时再抽"的规则。
- **`mw-cli`**：参数解析（clap）、文字和 JSON 输出、写文件，依赖 `mw-scripting`，不链接 GPUI。
- **`mw-python`**：PyO3 绑定，依赖 `mw-scripting`。Rust 库名是包名 `mw_python`，扩展模块名由 `#[pymodule(name = "materials_workbench")]` 和 maturin 的 `module-name` 给出：应用的可执行文件 `materials-workbench` 的 crate 名也是 `materials_workbench`，同名的库会在 Windows 上撞 `.pdb` 文件、在 `target/doc` 里撞目录。

没有复用 `mw_mcp::Headless`：它为 MCP 设计（允许的目录、项目文件、只读），命令行和 Python 要的是"读一个文件、改、写出"，用注册表自带的内存宿主 `Documents` 正合适。读写 `.mwproj` 项目留到以后（第 10 节）。

`mw-automation` 没有改：用到的只有现有的公开接口（`Registry`、`Documents`、`Host`、`CallResult`、`CallError`、`Effect`、`Availability`、`diagnostic_message`）和能力名。M18 改注册表时，这里只依赖能力名和输入输出的字段；M18 加了 `structure.make_supercell` 以后，`Document::supercell` 改为调用它。

## 4. 命令行 `mw`

### 4.1 形式

两种选择：

| | 独立的可执行文件 `mw`（`crates/mw-cli`） | 主程序的子命令（`materials-workbench convert …`，像 `mcp` 那样） |
|---|---|---|
| 依赖 | 只有领域 crate，不链接 GPUI、wgpu 和 TLS：依赖树 78 个 crate（应用 606 个），release 从零构建约 20 秒，可执行文件约 4 MB（2026-10-24 在 Apple 芯片的 macOS 上） | 整个应用：GPUI、wgpu、字体、TLS；Linux 上要图形库才能运行，服务器和 CI 容器里常常没有 |
| 安装 | 单独的文件，可以 `cargo install --path crates/mw-cli`，也可以单独打包 | 装了应用就有，不必另装 |
| 名字 | 短，适合在终端里反复敲 | 长，要写完整路径或另设别名 |
| 与 `mcp` 的一致性 | `mcp` 留在主程序里（它要读应用的配置目录、连接运行中的应用） | 所有命令行入口在一处 |

选独立的可执行文件：命令行的主要场景是服务器、CI 和批处理，那里不该需要图形库。代价是安装包要多带一个文件（M20 打包时一并处理）。可执行文件叫 `mw`（**待确认**，第 9 节）。

### 4.2 子命令

| 子命令 | 作用 | 调用 |
|---|---|---|
| `mw info FILE…` | 结构摘要：化学式与 Z、空间群与设定（P1 结构另写原子实际具有的对称性）、晶胞参数、体积、密度、位点数、原子数、操作数、编辑模式 | `structure.summary` |
| `mw diagnostics FILE…` | 每个文件读到的全部诊断，含导入失败的数据块，带行号；有导入失败的数据块或没有结构的文件时退出码为 1，可以当检查器用 | 见第 2 节 |
| `mw convert FILE… --to FORMAT` | 格式转换：读 CIF、POSCAR，写 CIF、POSCAR、扩展 XYZ | `Document::export` |
| `mw supercell FILE… --repeat 2x2x2` 或 `--matrix "1 1 0; -1 1 0; 0 0 1"` | 超胞（P1 的新结构），矩阵按行写，第 j 列是新的第 j 个基矢（与晶胞变换的 P 同一约定）；默认写 CIF | `make_supercell` |
| `mw transform FILE… --cell primitive\|conventional\|niggli`、`--setting "P 1 1 21/a"` 或 `--matrix "…" [--origin "…"]` | 晶胞变换，原子不动；默认写 CIF | `structure.transform_cell` |
| `mw symmetry FILE… [--tolerance Å] [--standardize]` | 对称性检测与对称化的预览，不写文件 | `structure.detect_symmetry` |
| `mw symmetrize FILE… [--tolerance Å] [--standardize]` | 对称化并写出；默认写 CIF | `structure.symmetrize` |
| `mw call CAPABILITY FILE… [--args JSON]` | 对每个结构调用一个只读能力，`--json` 给出它的结构化输出 | 注册表 |
| `mw capabilities` | 列出注册表的能力，`--json` 带上 Schema | 注册表 |

`mw call` 只接受只读能力：编辑能力改的是内存里的文档，命令结束就没了；要写出改过的结构，用 `transform` 和 `symmetrize`。注册表的编辑能力以后要批量用时，再给 `call` 加写出选项。

没有更高对称性（`symmetrize_no_higher_symmetry`）在注册表里是错误；在命令行里它是一个回答：`symmetry` 照常报告"没有更高的对称性"，`symmetrize` 把结构原样写出并注明，都不算失败，否则一批文件里只要有一个已经对称的就整体失败。

超胞最多 100 万个原子（`MAX_SUPERCELL_ATOMS`），超过时拒绝（`supercell_too_large`）：那样大的结构要几分钟分析、几百 MB 写出，`200x200x200` 更可能是打错了。

### 4.3 输入

- 文件名按第 2 节的规则认格式；`--from cif|poscar` 不看文件名，`-` 从标准输入读（必须给 `--from`，结构按数据块命名）。扩展 XYZ 只写不读。
- 一次可以给多个文件，每个文件的每个结构各处理一次。读不了的文件、导入失败的数据块（列出前 10 个错误和行号）、被拒绝的操作都单独报告，其余照常处理。
- 没有读 VASP 4 文件时补元素名的选项：界面也不读（M10 的诊断说明了要补什么）。

### 4.4 输出

- 写结构的子命令（`convert`、`supercell`、`transform`、`symmetrize`）：
  - 不给输出位置时写到标准输出，只允许一个结构；
  - `-o FILE` 写到一个文件，只允许一个结构；
  - `--out-dir DIR` 每个结构一个文件，文件名是结构名（` · ` 换成 `-`，文件系统不接受的字符和空白换成 `_`）加操作的后缀（`-2x2x2`、`-supercell`、`-primitive`、`-conventional`、`-niggli`、`-setting`、`-transformed`、`-symmetrized`）和格式的扩展名（`cif`、`vasp`、`xyz`）；目录不存在时创建。
- 格式：`--to`；不给时看 `-o` 的文件名；再不行，`convert` 报用法错误，其余默认 CIF（保留对称性）。
- 已存在的文件不覆盖，除非给 `--force`；同一次运行里两个结构落到同一个文件名时，第二个报错。
- 写 CIF 时源文件的字段因结构改过而丢掉的，在标准错误里警告（界面同样会警告）。

### 4.5 文字与 JSON

数据写标准输出，消息写标准错误。文字是英文，与注册表和 MCP 的说明一致；命令行暂不本地化。

- 文字：`info` 是对齐的表格；`symmetry`、`call` 和写结构时的说明直接用注册表的英文摘要（`CallResult::summary`），文字只有一个来源。
- `--json`（可以放在任何位置）：标准输出上只有一个 JSON 文档 `{"results": [...], "errors": [...]}`（`capabilities` 是 `{"capabilities": [...]}`）。每个结果带 `file`、`structure`，以及注册表的 `summary` 和 `output`；写结构的结果另有 `format`、`path`、`dropped_fields`、`atoms_in_cell`，写到标准输出时文本放在 `text` 里而不是直接打印；`diagnostics` 的结果是每个文件一项（`FileReport::diagnostics_json` 加 `file`）。每个错误有 `file`、`structure`、稳定的 `code`（注册表的错误码，或 `unreadable`、`unknown_format`、`import_failed`、`no_structure`、`exists`、`same_output`、`unwritable`）、`message`、`guidance`，导入失败时有 `diagnostics`。

### 4.6 退出码

- 0：全部完成；
- 1：有文件或结构失败（读不了、导入失败、操作被拒绝、目标文件已存在），其余的照常完成；`diagnostics` 遇到导入失败的数据块或没有结构的文件；
- 2：用法错误，包括读完文件才知道的（多个结构写到标准输出或 `-o`、`convert` 不知道写什么格式、`call` 的能力不存在或不是只读、`--args` 不是 JSON 对象、标准输入没有 `--from`），这时什么都不写。

## 5. Python 模块 `materials_workbench`

### 5.1 接口

模块名 `materials_workbench`（**待确认**，第 9 节），建议写 `import materials_workbench as mw`。结果是注册表输出的 JSON 转成的 dict 和 list，字段和 MCP 工具、`mw --json` 相同。

| 接口 | 作用 |
|---|---|
| `mw.read(path, format=None)` | 读文件里的第一个结构 |
| `mw.read_all(path, format=None)` | 读文件里的全部结构（如 CIF 的每个数据块） |
| `mw.read_text(text, format)` | 从文本读第一个结构，按数据块命名 |
| `mw.diagnostics(path, format=None)` | 读文件得到的全部诊断，导入失败时也给出 |
| `mw.capabilities()` | 注册表的能力和 Schema |
| `Structure.name`、`.file` | 结构名、来源文件名 |
| `.summary()`、`.sites()`、`.diagnostics()` | `structure.summary`、`structure.sites`、`structure.diagnostics` |
| `.detect_symmetry(tolerance=0.0, standardize=False)` | `structure.detect_symmetry` |
| `.symmetrize(tolerance=0.0, standardize=False)` | `structure.symmetrize`，原地、可撤销 |
| `.transform_cell(cell=None, *, setting=None, matrix=None, origin=None)` | `structure.transform_cell`，原地、可撤销；矩阵和原点的元素可以是整数、`"1/2"`、`fractions.Fraction` 或 0.5 这样的浮点数（注册表把小数读成它表示的分数，读不成的拒绝） |
| `.supercell(size)` | 新结构；`size` 是 2、`[2, 2, 1]` 或 3×3 矩阵 |
| `.export(format)`、`.write(path, format=None)` | 文件文本；写文件（格式默认按文件名，已有的文件照 Python 的 `open(path, "w")` 覆盖，与命令行不同） |
| `.call(capability, args=None)` | 按名字调用任意能力，`args` 是 dict 或 JSON 文本，`structure` 自动填上 |
| `.undo()`、`.redo()`、`.copy()` | 撤销、重做（返回是否有变化）、带撤销历史的独立副本 |

原地还是返回新结构，跟界面一致：晶胞变换和对称化是结构上的编辑，进撤销栈；超胞是新的派生结构。

### 5.2 错误

- `mw.WorkbenchError`：本模块所有错误的基类。
- `mw.CapabilityError`：能力拒绝了调用，结构不变；`code`、`message`、`guidance` 就是注册表的 `CallError`。
- `mw.ReadError`：认不出格式，或文件里没有能导入的结构；`diagnostics` 是 `mw.diagnostics()` 的结果，消息里列出前 10 个错误和行号。
- 读不了的文件抛 Python 自己的 `OSError` 子类（如 `FileNotFoundError`），带文件名；参数不对抛 `ValueError` 或 `TypeError`。
- 部分数据块导入失败而其余成功时，`read`、`read_all` 发 `UserWarning`；写 CIF 时丢掉源文件字段也发警告。

### 5.3 线程

计算量大的调用（读文件、对称性检测与对称化、晶胞变换、超胞、写出、`call`）期间释放 GIL（`Python::detach`），别的 Python 线程可以继续运行；同一个 `Structure` 同时只能有一个调用。

### 5.4 JSON 与 Python 对象

注册表的输出经 `serde_json` 写成文本，再用 Python 的 `json.loads` 变成 dict 和 list；`call` 的参数反过来经 `json.dumps`。浮点数两边都按最短往返的写法，不丢精度；不需要另一个依赖（`pythonize` 要跟 PyO3 的版本走）。

### 5.5 构建：特性怎么配

- PyO3 0.30（MIT OR Apache-2.0），特性 `abi3-py310`：一个 wheel 适用于 CPython 3.10 及以后的所有版本。
- `mw-python` 的 PyO3 依赖是可选的：
  - `python` 特性打开 PyO3 和绑定；
  - `extension-module` 特性在它之上打开 PyO3 的 `extension-module`，不链接 libpython（Linux 和 macOS 上扩展模块不能链接它）。
  
  两者默认都关，所以 `cargo build`、`cargo test --workspace` 和 `cargo clippy --workspace` 只编译不依赖 Python 的部分（参数的判断和它们的单元测试），没装 Python 的机器上照常通过。PyO3 链接 libpython 时要找到 Python，`extension-module` 在测试可执行文件上又会留下找不到的符号，所以不能默认打开。
- PyO3 0.26 起推荐用环境变量 `PYO3_BUILD_EXTENSION_MODULE` 代替 `extension-module` 特性，maturin 1.9.4 起会自动设；这里仍用特性，手工构建（冒烟测试）和较旧的 maturin 都不必另设环境变量。
- macOS 的链接器要被告知 Python 的符号由加载模块的解释器提供：`build.rs` 在打开 `python` 特性时给 cdylib 加 `-undefined dynamic_lookup`（与 `pyo3_build_config::add_extension_module_link_args` 相同，maturin 也会加）。
- Windows 上 PyO3 用 `raw-dylib` 链接，不需要 Python 的导入库。
- 打包用 maturin：`crates/mw-python/pyproject.toml`，发行名 `materials-workbench`，`module-name = "materials_workbench"`，`features = ["extension-module"]`，版本取 `Cargo.toml`。

### 5.6 测试

- Rust 侧：`mw-python` 的单元测试检查参数的判断（超胞的三种写法与范围、晶胞变换的选择与矩阵形状、分数的文本、按名字和文件名认格式），不需要解释器；查询、编辑、超胞、写出、诊断与注册表和领域函数直接调用的结果一致，由 `mw-scripting` 的单元测试保证。
- Python 侧：`crates/mw-python/tests/smoke.py` 把 `cargo build -p mw-python --features extension-module` 构建的动态库复制到临时目录、改名为 Python 能导入的名字（`materials_workbench.so`，Windows 上 `.pyd`），加进 `sys.path` 后导入，在 `mw-crystal-io` 的测试文件上检查每个接口、每种错误，并在给出 `mw` 可执行文件时检查命令行写出同样的文本。用 `python -I` 运行：它忽略 `PYTHONPATH` 和用户目录里的包，所以目录由脚本自己加。不用 pip、不装 maturin。
- `--installed` 模式检查装在当前 Python 里的模块，CI 用它测试构建出的 wheel。

### 5.7 发布

wheel 在 CI 里为三个平台构建（第 6 节），作为产物保存。是否发布到 PyPI、以什么名字发布（`materials-workbench` 在 PyPI 上是否可用还没查）由你决定（**待确认**，第 9 节），这次不发布。发布前还要：版本号（现在是 `0.0.0`）、README 和类型存根（`.pyi`）、Linux aarch64 和 musl 的 wheel。

## 6. CI

`.github/workflows/ci.yml`：

- `check` 任务（三个平台）在原有步骤之后：
  - 用 `actions/setup-python` 装 Python 3.13；
  - 静态检查：`cargo clippy -p mw-python --features extension-module --all-targets --locked -- -D warnings`；
  - 冒烟测试：构建 `mw` 和扩展模块，运行一次 `mw info`，再运行 `smoke.py`（带上 `mw`）。
  
  命令行的测试已经在 `cargo test --workspace` 里。
- `wheels` 任务（三个平台）：
  - `PyO3/maturin-action` 在 `crates/mw-python` 里构建 release wheel：macOS 是 universal2（Apple 芯片和 Intel），Linux 在 manylinux 容器里构建，Windows x64，Rust 1.97；
  - 把 wheel 装进一个新的虚拟环境（`pip install --no-index --no-deps`，只装这个文件），用 `smoke.py --installed` 检查；
  - wheel 作为产物保存。

工作流在本地无法运行，推送后才知道结果（开发指南 2.1 节）。

## 7. 与界面一致的测试

- `mw-scripting` 的单元测试：摘要、对称性检测（包括拒绝）与注册表直接调用的结果相同；每个测试文件写成三种格式的文本与 `structure.export` 相同（包括拒绝时的错误码）；超胞与 `make_supercell` 逐位相同，名字是界面的名字；晶胞变换和对称化可以撤销重做；分页的能力取全；诊断与 `structure.diagnostics` 相同；按文件名认格式与界面的规则相同。
- `mw-cli` 的集成测试（`crates/mw-cli/tests/cli.rs`）运行 `mw` 可执行文件，结果与直接用 `mw-crystal-io` 读文件、在 `Documents` 里调用注册表或领域函数的结果比较：`info` 与 `structure.summary`；`convert` 的三种格式与三个写出函数（7 个测试文件）；`supercell` 与 `make_supercell`；`transform` 的预设、矩阵加原点、设定与 `structure.transform_cell`；`symmetry`、`symmetrize` 与 `structure.detect_symmetry`、`structure.symmetrize`；`diagnostics` 与 `structure.diagnostics`；`call` 与 `structure.sites`；`capabilities` 的数量。另测：一批文件写进目录、文件名、不覆盖与 `--force`、按 `-o` 的文件名认格式、标准输入；错误输入的消息和退出码（读不了的文件、认不出的格式、导入错误带行号、部分占位写 POSCAR、多个结构写到一处、缺格式、错误的格式名、`--from xyz`、错误的倍数和矩阵、互斥的选项、`--origin` 没有 `--matrix`、行列式为 0 的超胞、不成立的变换、JSON 模式下的错误）。
- Python 的冒烟测试见 5.6 节。

## 8. 没有做的

- 读写 `.mwproj` 项目（在项目里批量导出、把结果加进项目）：要先定下命令行写项目时的锁与备份，和 M18 无界面写项目文件一起考虑。
- 转为 P1 没有单独的子命令：注册表还没有 `structure.convert_to_p1`（M18）；POSCAR 和扩展 XYZ 写出时本来就展开为 P1。
- 读扩展 XYZ、给 VASP 4 文件补元素名。
- Python 的类型存根、numpy 数组形式的坐标和晶格、Jupyter 里的富显示（`_repr_html_`）。
- 命令行文字的本地化。

## 9. 需要确认的决定

1. **命令行的形式**：独立的可执行文件，名字 `mw`（4.1 节）。另一个选择是主程序的子命令。
2. **Python 模块名** `materials_workbench`、发行名 `materials-workbench`（5.1、5.7 节）。
3. **是否发布到 PyPI**，以及何时发布（5.7 节）；这次只在 CI 里构建 wheel。
