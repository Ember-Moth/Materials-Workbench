# 3D 视口：技术选型与 GPUI 集成

> 状态：已决定 · macOS 上已用 PoC 验证方案 A（见 8.1 节），Linux / Windows 待测 · 2026-10-22
> 相关：[晶体学模块：数据模型与编辑语义](crystallography-model.md)（`mw-crystal-render` 持有的视图状态，以及拾取结果如何映射到 `AtomRef`）
> 源码结论基于 GPUI Kit 0.8.0 锁定的 `gpui-pre 0.3.9` 系列 crate，于 2026-10-22 从 crates.io 下载后逐一核对。升级 GPUI Kit 时，按附录清单重新核对。

## 1. 决定

| 项 | 决定 |
|---|---|
| UI 框架 | GPUI，通过 GPUI Kit 使用（应用只依赖 `gpui-kit` 一个 crate） |
| 3D 渲染 | wgpu，版本与 GPUI 的 wgpu 渲染器对齐（当前为 29.x） |
| 渲染器边界 | `mw-crystal-render` 只依赖 wgpu，不依赖 GPUI |
| 视口接入 GPUI | 第一阶段：离屏渲染 → 回读到内存 → `Window::paint_image`，全平台通用。之后根据实测结果，在 macOS / Linux 上改为给 GPUI 打补丁的零拷贝路线 |
| 原生子视图 | 不采用 |
| Bevy | 不采用 |

## 2. 约束

视口不是一块孤立的画布，它必须和工作台的其余部分融为一体：

- **GPUI 弹层必须能盖在视口上**：命令面板、右键菜单、悬停提示（原子信息）、测量标注、对话框。
- **文字由 GPUI 绘制**：原子标签和测量值要和界面使用相同的字体、支持中文、跟随主题色。
- **平台与显示**：macOS、Windows、Linux 三个平台都要支持；HiDPI 下像素精确，不发虚。
- **按需渲染**：画面不变时，GPU 和 CPU 占用接近 0。
- **Web**：将来可能做网页版查看器（GPUI Kit 有 WebAssembly 目标）。

## 3. 源码核查结论

### 3.1 各平台的 GPUI 渲染器

`gpui-pre-platform 0.3.9` 的 `Cargo.toml` 按目标平台选择后端：

| 平台 | 后端依赖 | 图形 API |
|---|---|---|
| macOS | `gpui_macos` → `gpui_apple::metal_renderer` | Metal |
| Linux | `gpui_linux` + `gpui_wgpu`（包名 `gpui-pre-wgpu`） | wgpu 29（Vulkan 等） |
| Windows | `gpui_windows`（包名 `gpui-pre-windows`） | Direct3D 11 + DirectComposition |
| Web | `gpui_web` | wgpu（优先 WebGPU，失败时退回 WebGL2） |

### 3.2 外部 GPU 画面能否直接显示

GPUI 有一个 `Surface` 图元，但它是为通话视频设计的：

| 位置 | 结论 |
|---|---|
| `gpui-pre`：`src/elements/surface.rs:35` 的 `surface()`，`src/window.rs:5155` 的 `Window::paint_surface` | 都带 `#[cfg(any(target_os = "macos", target_os = "ios"))]`，其他平台没有这个 API |
| `gpui-pre`：`src/scene.rs:768` 的 `PaintSurface` | 内容字段是 `CVPixelBuffer`，同样只在 macOS / iOS 上存在 |
| `gpui-pre-apple`：`src/metal_renderer.rs:1219-1221` | 断言像素格式必须是 `kCVPixelFormatType_420YpCbCr8BiPlanarFullRange`（YUV 4:2:0 双平面）。1245-1246 行分别把 Y 平面按 `R8Unorm`、CbCr 平面按 `RG8Unorm` 采样 |
| `gpui-pre-wgpu`：`src/wgpu_renderer.rs:1614` | 绘制分支为 `PrimitiveBatch::Surfaces(_surfaces) => {}`，旁边的注释说明 Surface 只用于 macOS 视频播放，wgpu 渲染器没有实现。不过 surfaces 管线和绑定布局已经创建（第 136、163、567、920 行） |
| `gpui-pre-windows`：`src/directx_renderer.rs:816` | `draw_surfaces` 是空实现 |

**结论：现有 GPUI 在任何平台上都不能零拷贝显示外部渲染的 RGB 画面。** macOS 的 Surface 只接受 YUV 4:2:0；把 3D 画面转成 YUV，细键、边缘和文字会出现色彩失真，不可接受。

### 3.3 可用的通用接口

| 位置 | 用途 |
|---|---|
| `gpui-pre`：`src/window.rs:5053` 的 `Window::paint_image` | 绘制一张 `Arc<RenderImage>`，全平台可用 |
| `gpui-pre`：`src/window.rs:5175` 的 `Window::drop_image` | 把图像从精灵图集中移除。每帧换新图时必须调用，否则图集会持续增长 |
| `gpui-pre`：`src/assets.rs:42-43` 的 `RenderImage` | 文档注释写明数据是 BGRA 格式 |
| `gpui-pre`：`src/window.rs:6926` 的 `Window::gpu_specs`，返回 `src/gpui.rs:393` 的 `GpuSpecs` | 提供 GPUI 所用 GPU 的 `device_name` 和 `driver_name`，可以用来让 wgpu 选同一块 GPU |
| `gpui-pre-wgpu`：`src/wgpu_context.rs:14-15` | `WgpuContext` 有 `pub device` 和 `pub queue`，但没有通过 `Window` 暴露给应用 |

### 3.4 GPUI Kit 文档中的相关限制

- **原生子视图**（`docs/native-extension.md`，即 `gpui-webview` 的实现方式）：
  - 子视图位于 GPUI 内容之上，GPUI 的弹出层、对话框和提示无法可靠地盖住它。
  - Windows 上要在启动前设置 `GPUI_DISABLE_DIRECT_COMPOSITION=true`。
  - Linux 上只有 X11 / XWayland 有可行路径。
- **渲染测试**（`docs/test.md`）：GPUI Kit 的像素级渲染测试覆盖 macOS（Metal）和 Linux（WGPU）；Windows 没有无头渲染器。
- **WebAssembly**（`docs/webassembly.md`）：只有一个 canvas、一个顶层窗口；WebGPU 失败时退回 WebGL2。

## 4. 方案比较

| 方案 | 做法 | 弹层能否盖住视口 | 平台 | 主要代价 | 结论 |
|---|---|---|---|---|---|
| A 回读 | wgpu 离屏渲染 → 拷到 CPU → `paint_image` | 能 | 全部 | 每帧一次 GPU→CPU→GPU 往返 | **第一阶段采用** |
| B 原生子视图 | 在 GPUI 窗口中嵌入原生 view，wgpu 直接画在上面 | 不能 | macOS、Windows、Linux X11 | 弹层被挡；Wayland 不可用；Windows 要关闭 DirectComposition | 不采用 |
| C 打补丁零拷贝 | 给 GPUI 加一个"外部纹理"图元 | 能 | 逐个平台实现 | 需要维护 `gpui-pre` 补丁 | **视实测结果，第二阶段采用** |
| D GPUI 原生绘制 | 用 `paint_path` 和图片精灵画 2.5D 球棍模型，在 CPU 上按深度排序 | 能 | 全部 | 没有真实深度，相交处画错，大结构性能差 | 不作为主视口，可用于缩略图 |
| E Bevy | 用 Bevy 渲染 | 同方案 A / C | — | 见第 7 节 | 不采用 |

**方案 A 的带宽估算**：每帧字节数 = 宽 × 高 × 4。

| 视口物理像素 | 每帧数据量 | 60 fps 时的单向带宽 |
|---|---|---|
| 1280 × 800 | 约 4 MB | 约 0.25 GB/s |
| 2560 × 1600 | 约 16 MB | 约 1 GB/s |
| 3840 × 2160 | 约 33 MB | 约 2 GB/s |

每帧回读、上传各要走一次。缓解手段见 5.3 节，实际开销以 PoC 实测为准。

## 5. 选定架构

### 5.1 crate 边界

```text
mw-crystal          结构模型与派生数据（见晶体学设计稿）
   │
mw-crystal-render   只依赖 wgpu：场景构建、渲染管线、拾取、相机
   │                输入：wgpu Device / Queue + 结构快照 + 相机 + 视图状态
   │                输出：颜色纹理 + 拾取 ID 纹理（+ 深度）
   │
apps/desktop        视口呈现层
   ├── 回读呈现       方案 A，全平台
   └── 共享纹理呈现   方案 C，按平台逐个加入
```

- **渲染器完全不知道 GPUI 的存在**，可以无头运行：渲染到纹理，再与参考图比对，作为渲染回归测试。这条路不依赖 GPUI 的无头渲染器，所以 Windows 上也能跑。CI 上可以用软件适配器（Linux 用 lavapipe，Windows 用 WARP），这一点需要验证。
- **呈现层只负责"把一张纹理放进 GPUI 布局"**。在方案 A 和 C 之间切换，不影响渲染器。

### 5.2 设备与适配器

- 方案 A 下，渲染器自己创建 wgpu 的 Instance、Adapter 和 Device。
- **适配器选择**：用 `Window::gpu_specs()` 返回的 `device_name` 去匹配 wgpu 的适配器信息，尽量和 GPUI 用同一块 GPU，因为双显卡笔记本上跨卡拷贝更慢。匹配不上时，退回 `HighPerformance` 偏好。
- **wgpu 版本锁定**为和 `gpui-pre-wgpu` 相同的大版本（当前是 29）。这样依赖树里只有一份 wgpu；这也是方案 C 在 Linux / Web 上共用设备的前提。

### 5.3 方案 A 的帧流程

1. **只在需要时渲染**：输入事件（拖拽旋转、缩放）更新相机，并把视口标记为"需要重绘"。画面没有变化就不渲染。
2. **离屏渲染**：渲染器渲染到离屏纹理（MSAA 解析之后），同时渲染一张拾取 ID 纹理。
3. **拷贝到暂存缓冲**：用 `copy_texture_to_buffer`。每行的字节数要按 wgpu 的 256 字节对齐（`COPY_BYTES_PER_ROW_ALIGNMENT`）补齐，读出时再去掉补齐的部分。
4. **异步映射**：`map_async` 映射缓冲，**绝不在 UI 线程上阻塞等待**。暂存缓冲准备 2–3 个轮流使用。
5. **交给 GPUI**：映射完成后构造 BGRA 格式的 `RenderImage`，通知视图重绘。在 paint 阶段调用 `paint_image`，同时对上一帧的图像调用 `drop_image`。

**分辨率策略**：

- 交互进行中，按缩放系数（默认 0.5，可调）渲染，由 GPUI 拉伸显示。
- 停止交互约 150 ms 后，补渲染一帧全分辨率画面。
- 静止的画面不再重绘。

### 5.4 拾取

- 拾取与呈现方式无关。渲染器维护一张 `R32Uint` 格式的 ID 纹理，点击或悬停时只回读鼠标下的 1 个像素（或一个小区域）。
- ID 编码可以映射到 `AtomRef`（位点、轨道成员、晶胞偏移），也可以映射到键、晶胞边、多面体等其他可选对象。
- 悬停拾取同样走异步映射，结果晚一帧到达是可以接受的。

### 5.5 文字与标注

原子标签、测量数值和坐标轴标签都由 GPUI 画在视口之上：

1. 渲染器提供相机矩阵。
2. 呈现层把 3D 锚点投影到视口坐标。
3. GPUI 的文本系统在这些坐标上绘制文字。

这样字体、中文和主题色都与界面一致，HiDPI 下也清晰。标签被原子遮住时，根据拾取 ID 纹理或深度判断是否隐藏。

### 5.6 渲染技术（第一版）

- **原子**：impostor 球——画一个面向屏幕的四边形，在片元着色器里做光线求交并写入正确的深度，用一次实例化绘制完成。
- **键**：impostor 圆柱，同样写入正确的深度。
- **晶胞边框与晶格平面**：分别用线段和半透明面绘制。
- **多面体**：半透明。先用排序处理，必要时再换成加权混合的顺序无关透明（OIT）。
- **抗锯齿**：MSAA 4×。环境光遮蔽（SSAO）作为可选效果，以后再加。

## 6. 方案 C：各平台的零拷贝路线

只有当 PoC 证明方案 A 不够用时才实施，并且按平台独立推进：

| 平台 | 做法 | 需要改的 crate | 难度 |
|---|---|---|---|
| Linux / Web | 通过 `Window` 暴露 GPUI 的 wgpu `Device` / `Queue`；新增"wgpu 纹理"图元，复用 wgpu 渲染器里已经创建好的 surfaces 管线，画一个带纹理的四边形 | `gpui-pre`、`gpui-pre-wgpu` | 低：同一个设备、同一个队列，不需要跨 API 同步 |
| macOS | 渲染器（wgpu 的 Metal 后端）渲染到由 IOSurface 支持的纹理；在 GPUI Metal 渲染器的 `draw_surfaces` 里，加一个 `kCVPixelFormatType_32BGRA` 单平面格式的分支 | `gpui-pre`、`gpui-pre-apple` | 中：两个 Metal 设备之间要做同步，IOSurface 要轮换；还需确认 wgpu 29 的 hal 互操作接口 |
| Windows | wgpu（D3D12）渲染到可共享的资源，GPUI（D3D11）打开共享句柄后绘制 | `gpui-pre`、`gpui-pre-windows` | 高：要跨 API 共享资源并用围栏同步。默认继续使用方案 A |

**补丁的维护策略**：

- 用 `[patch.crates-io]` 指向自己维护的 `gpui-pre-*` 分支。
- 补丁尽量小，争取提交给上游（Zed / GPUI Kit）。
- 每次升级 GPUI Kit，都按附录清单复核。

## 7. 不采用 Bevy 的理由

- **嵌入问题不会消失**：Bevy 要自己掌控事件循环和窗口，放进 GPUI 一样要走方案 A 或 C。
- **wgpu 版本很可能对不上**：Bevy 的 wgpu 版本跟着 Bevy 版本走，很可能和 GPUI 的 wgpu 29 不一致。这样就无法共用设备，依赖树里还会出现两份 wgpu。
- **会出现两份状态**：Bevy 的 ECS 世界和文档模型是两份数据，需要持续同步，这和"结构是唯一事实来源"的设计冲突。
- **能力用不上**：晶体渲染需要的是专用管线（impostor、拾取 ID、标注投影）。Bevy 的 PBR 材质、场景和资源系统基本用不上，却要承担它的编译时间、二进制体积和每个大版本的升级成本。

## 8. PoC 计划

**范围**：一个最小的 GPUI Kit 应用，左侧是面板，中间是视口，右侧是属性面板。视口渲染合成的球体阵列（1 千、1 万、10 万个原子），可以用鼠标拖拽旋转。不接入真实的晶体模型。

**测量项**（先测 macOS，再测 Linux 和 Windows）：

| 指标 | 条件 |
|---|---|
| 每帧耗时分解：渲染、拷贝与映射等待、构造 `RenderImage`、GPUI 绘制 | 视口物理分辨率分别为 1280×800、2560×1600、3840×2160 |
| 连续旋转时的帧率和输入延迟（以帧计） | 交互缩放系数分别为 1.0 和 0.5 |
| 连续旋转 10 分钟时的图集内存和进程内存 | 必须保持平稳 |
| 静止时的 CPU / GPU 占用 | 应接近 0 |

**功能检查**：

- 命令面板、右键菜单、Popover、Tooltip 盖在视口上时显示正常。
- HiDPI 下像素精确；缩放窗口、在缩放比不同的显示器之间拖动窗口都正常。
- 深浅色主题切换正常；双显卡机器上适配器匹配正确。
- 测出 1 像素拾取回读的延迟。

**验收建议**（阈值待 PoC 后确认）：在 2560×1600、1 万个原子、交互缩放系数 0.5 的条件下，如果连续旋转能稳定在 60 fps，并且停止交互后 150 ms 内能出全分辨率帧，该平台就保持方案 A；达不到时，该平台进入方案 C。

### 8.1 macOS 实测结果（2026-10-22）

代码：渲染器在 `crates/mw-crystal-render`，PoC 应用在 `apps/viewport-poc`。复现命令：

```bash
VIEWPORT_POC_BENCH=1 cargo run -p viewport-poc
```

**环境**：

- Apple M5（统一内存），内建屏刷新率 60 Hz，所以帧率上限就是 60 fps。
- GPUI Kit 0.8.0 / `gpui-pre` 0.3.9，wgpu 29.0.4（Metal 后端）。
- debug 构建，但 GPUI 和 wgpu 按 GPUI Kit 安装指南以 `opt-level = 3` 编译。
- 窗口 1440×900 逻辑像素，视口 896×800 逻辑像素，即 1792×1600 物理像素。
- 自动旋转，每档预热 1 秒、测量 5 秒；时间是测量期间的平均值，单位 ms。

| 原子数 | 渲染尺寸 | 每帧数据 | 显示帧率 | 编码提交 | GPU + 回读等待 | 内存拷贝 | GPUI 绘制（含上传） | 请求 → 出图 |
|---|---|---|---|---|---|---|---|---|
| 1 千 | 1792×1600 | 11.5 MB | 60 | 0.40 | 4.3 | 1.08 | 2.42 | 5.8 |
| 1 千 | 896×800 | 2.9 MB | 60 | 1.03 | 5.1 | 0.29 | 0.86 | 6.4 |
| 1 万 | 1792×1600 | 11.5 MB | 60 | 0.41 | 4.4 | 0.93 | 2.32 | 5.7 |
| 1 万 | 896×800 | 2.9 MB | 60 | 0.95 | 4.9 | 0.28 | 0.87 | 6.2 |
| 10 万 | 1792×1600 | 11.5 MB | 60 | 0.43–0.45 | 7.6–9.4 | 0.92–0.96 | 1.90–2.03 | 9.0–10.8 |
| 10 万 | 896×800 | 2.9 MB | 60 | 1.01 | 5.1 | 0.28 | 0.83 | 6.5 |

10 万原子全分辨率一行是两轮测量的范围。

**结论**：

- **所有档位都跑满了显示刷新率。** UI 线程每帧花在视口上的时间约 1–3 ms（编码提交 + GPUI 绘制上传），等待和拷贝都在后台线程。
- **从相机变化到新图像就绪 6–11 ms**，大约比直接上屏多一帧延迟。
- **"GPU + 回读等待"有一个 4–5 ms 的下限**，和分辨率关系不大，主要是从提交到完成的调度延迟；只有 10 万原子全分辨率时它才明显上升。
- **按上面的验收建议，macOS 保持方案 A，暂不需要方案 C。** 验收条件要求的渲染尺寸（2560×1600 缩放 0.5，即 1280×800）比本次的 896×800 略大，但本次全分辨率 1792×1600 也稳定 60 fps。
- **外推（未实测）**：每帧数据量与像素数成正比。2560×1600 全分辨率约 16.4 MB，按本次数据线性外推，拷贝约 1.4 ms、绘制上传约 3.4 ms，仍在 60 Hz 的 16.7 ms 预算内；120 Hz（8.3 ms 预算）下全分辨率会比较紧，交互时降到 50% 即可。

**测量中发现并修复的问题**：

- **多余的渲染**：每次重绘都登记一次"下一帧推进旋转"的回调，导致一帧内推进多次，完成的帧比显示的多 20–60%。现在同一时刻只登记一个回调，完成数与显示数一致。
- **掉帧**：前两轮在最后一档（10 万原子、50%）掉到 8.5 fps 和 1.5 fps。第三轮把这一档放到最前面、窗口一直在前台，稳定 60 fps。判断是测到最后时窗口被遮挡或切到后台，系统降低了刷新；前两轮没有记录窗口状态，不能完全确认。基准模式现在会记录测量期间窗口是否失去过焦点（`window_lost_focus`）。

**还没有验证的**：

- GPUI 弹层（Tooltip、Popover）盖在视口上的实际效果、HiDPI 下是否发虚、缩放窗口、跨显示器拖动。这些需要人工在窗口里检查（终端没有屏幕录制权限，无法自动截图）。渲染器本身的输出已用 `cargo run -p mw-crystal-render --example snapshot` 存成 PNG 检查过。
- 120 Hz 显示器，以及 2560×1600 以上的视口。
- Linux、Windows。
- 双显卡机器上的适配器匹配（本机只有一块 GPU）。
- 拾取（ID 纹理）还没有实现。

## 9. 风险与待确认

| 项 | 说明 |
|---|---|
| `RenderImage` 的颜色约定 | 注释写的是 BGRA，还需要确认 sRGB 和预乘 alpha 的约定，避免颜色偏差 |
| 每帧新建 `RenderImage` 的图集开销 | `paint_image` 走精灵图集，需要确认高频替换时的上传开销和碎片情况 |
| GPUI Fast 后端 | GPUI Kit 的可选特性 `gpui-fast` 没有评估，不在本次选型范围内 |
| 方案 C 能否进入上游 | 如果补丁进不了上游，就要长期跟着 GPUI Kit 的版本维护 |

## 附录：升级 GPUI Kit 时的复核清单

升级后，在新版 `gpui-pre-*` 的源码中检查以下几点：

1. `surface()` / `Window::paint_surface` 是否仍然只限 macOS / iOS，`PaintSurface` 的内容类型有没有变。
2. `gpui-pre-apple` 的 `draw_surfaces` 是否仍然只接受 YUV 4:2:0。
3. `gpui-pre-wgpu` 和 `gpui-pre-windows` 的 Surface 绘制是否仍然是空实现。
4. `gpui-pre-wgpu` 依赖的 wgpu 大版本——它决定 `mw-crystal-render` 用哪个版本的 wgpu。
5. `Window::paint_image`、`Window::drop_image` 和 `RenderImage` 的签名与格式说明有没有变化。
6. 是否出现了新的外部纹理或自定义渲染接口。如果有，重新评估方案 C。
