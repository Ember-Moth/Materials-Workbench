# 3D 视口：底层合成（方案 F）可行性报告

> 状态：macOS 已做原型并实测；Windows、Linux 只做了源码核查，**未实测** · 2026-10-23
> 相关：[3D 视口：技术选型与 GPUI 集成](viewport-rendering.md)（方案 F 的设想在 6.2 节，方案 A 的实测在 8.1、8.2 节）
> 原型代码：只在分支 `claude/underlay-feasibility-5c0830` 上，不合并进 main。下文提到的 `apps/viewport-poc` 的方案 F 模式、`src/underlay.rs` 和 `Renderer::render_to` 都在那个分支里。
> 源码依据：`Cargo.lock` 锁定的 GPUI Kit 0.8.0、`gpui-pre-*` 0.3.9、`gpui-component` 0.8.0、`gpui-base` 0.8.0、`wgpu`/`wgpu-hal`/`wgpu-types` 29.0.4、`raw-window-metal` 1.1.0、`wayland-client` 0.31.15，位于 `~/.cargo/registry/src/index.crates.io-*/`。文中“文件:行号”都指这些 crate。

## 1. 结论

| 平台 | 判断 | 依据 | 是否实测 |
|---|---|---|---|
| macOS | **可行，有风险** | GPUI 的视图只是窗口内容视图的一个子视图，插一个兄弟视图到它下面就行，不用碰系统私有视图，也不影响事件、拖放、输入法和第一响应者；透明背景下 GPUI 清成 alpha 0；wgpu 不覆盖 `presentsWithTransaction`。原型里弹层、通知、对话框都正确盖在 3D 画面上，静止和旋转时两层完全对齐。**风险**：GPUI 只在拖动改变窗口大小时用 Core Animation 事务送显，平时异步送显，外部图层没法和它原子提交，侧栏动画时约 1.5% 的帧边缘错开一帧；取 drawable 有时在界面线程上阻塞 7–15 ms | 是（本机，Apple M5） |
| Windows | **源码上可行，同步方式未知** | GPUI 只建了顶层（topmost）DirectComposition 目标，窗口还能再建一个非顶层目标，系统把它合成在下面；GPUI 的合成交换链总是预乘 alpha，透明时清成 0；wgpu 29 能在自己的 DirectComposition 视觉对象上建交换链。两个设备各自提交，没有跨设备的原子提交，预计和 macOS 平时一样有偶发错帧 | 否 |
| Linux Wayland | **源码上可行，同步条件最好** | GPUI 暴露 `wl_surface` 和 `wl_display`，用的是系统 libwayland，可以在同一连接上另开事件队列；子表面默认是同步模式，位置和层叠次序都是父表面的双缓冲状态，和 GPUI 的下一次提交原子生效。要自己绑定 `wl_subcompositor`，并处理分数缩放和 Vulkan 送显节流 | 否 |
| Linux X11 | **不可行** | 子窗口总是盖在父窗口自己的内容上面，没有“放在父窗口内容下面”的位置 | 不需要 |

**建议：现在不采用。** macOS 上方案 F 的收益（每帧省掉回读和上传，延迟少约一帧）是真的，但要付出透明窗口带来的布局约束、偶发错帧，还要另做渲染线程解决取 drawable 的阻塞；而方案 A 在当前目标下够用（`viewport-rendering.md` 8.1、8.2 节）。启动条件和顺序见第 7 节：先 Wayland，再 Windows，macOS 优先等上游 #61291。

## 2. 范围

方案 F 的设想见 `viewport-rendering.md` 6.2 节：wgpu 渲染到一个原生子图层，图层放在 GPUI 内容下面；GPUI 窗口用透明背景，只在视口处留洞；由系统合成器把两层叠起来。本报告回答三件事：

1. 每个平台上，图层放在哪里、怎样和 GPUI 同步，依据是什么（第 3 节）；
2. macOS 原型实际表现如何（第 4 节）；
3. 值不值得做、什么时候做（第 5–7 节）。

## 3. 源码核查

### 3.1 macOS

**视图层级。** GPUI 用的是 `NSWindow` 自带的内容视图，自己的 `GPUIView` 只是它的一个子视图：

| 位置（`gpui-pre-macos` `src/window.rs`） | 内容 |
|---|---|
| 1087–1089 | `content_view = native_window.contentView()`；`GPUIView` 以内容视图的 `bounds` 初始化 |
| 1192–1200 | 自动随内容视图缩放；`setWantsLayer(YES)`；重绘策略 `DuringViewResize`；`content_view.addSubview_(native_view)`；`makeFirstResponder_(native_view)` |
| 1886–1899 | 模糊背景时，GPUI 自己就是用 `addSubview:positioned:NSWindowBelow relativeTo:nil` 把一个视图插到内容视图最底层 |
| 937 | 只读内容视图的 `frame` 求内容大小 |

GPUI 从不遍历内容视图的子视图，也不替换内容视图（全文没有 `subviews`、`setContentView`）。所以 6.2 节担心的“只能改窗口的内容视图或系统私有的外层视图”并不成立：**在内容视图里、`GPUIView` 下面插一个兄弟视图即可**，和 GPUI 自己插模糊视图的做法一样。

**事件、拖放、输入法、第一响应者。**

- 鼠标：AppKit 从上往下做命中测试，`GPUIView` 铺满内容视图且没有重写 `hitTest:`，所以下面的兄弟视图永远拿不到鼠标事件。
- 拖放：GPUI 把拖放类型注册在窗口上（1076–1081），不在视图上；我们的视图不注册，不参与。
- 输入法：`NSTextInputClient` 实现在 `GPUIView` 上（`build_classes` 里的协议和方法），与兄弟视图无关。
- 第一响应者：GPUI 显式设 `GPUIView`（1200）；普通 `NSView` 的 `acceptsFirstResponder` 默认是 `NO`。
- 原型实测：拖动旋转、按钮、弹出层、提示都和方案 A 一样工作。

**透明背景时没画东西的区域是否真的透明。**

| 位置 | 内容 |
|---|---|
| `src/window.rs` 1863–1880 | `set_background_appearance`：非不透明时 `setOpaque(NO)`，背景色 alpha 设为 0.0001（注释：用 `clearColor` 会弄坏阴影），并调用渲染器的 `update_transparency` |
| `gpui-pre-apple` `src/metal_renderer.rs` 177–200、481–486 | Metal 图层 `set_opaque(!transparent)` |
| 同上 722–729 | 每帧清屏颜色 `(0, 0, 0, alpha)`，透明时 alpha 为 0 |
| 同上 1362–1368 | 普通图元的混合：RGB 用 `SourceAlpha / OneMinusSourceAlpha`，alpha 用 `One / One`。画在 alpha 0 上得到的是预乘结果（RGB = c·a，alpha = a），Core Animation 按预乘合成，所以圆角、阴影的半透明边缘会正确地叠在 3D 画面上。两个半透明图元重叠处 alpha 会偏大（`One/One` 而不是 `One/OneMinusSourceAlpha`），那里的 3D 画面会被压得略暗，影响很小 |

另外一处必须处理：**GPUI Kit 的根视图会在整个窗口画主题背景**。`gpui-component` `src/root.rs` 543–550 的 `RootPlugin::style` 给根元素设 `bg(theme.background)`；`gpui-base` `src/root.rs` 241–270 在应用完插件样式之后再应用 `Root` 自己的样式，所以构造根视图时设 `.bg(transparent_black())` 就能清掉（原型 `main.rs`）。此外，从根元素到视口的每一层都不能画背景，每个区域改为自己画背景。

**`presentsWithTransaction`。**

| 位置 | 内容 |
|---|---|
| `metal_renderer.rs` 429–434、525–532 | GPUI 自己的开关：为真时 `commit` → `waitUntilScheduled` → `drawable.present()`（随 Core Animation 事务生效）；为假时 `present_drawable` 后 `commit`（GPU 完成后异步上屏） |
| `src/window.rs` 3321–3340 | 只在 `displayLayer:` 里（即拖动改变窗口大小时）打开，画完一帧马上关掉 |
| `src/window.rs` 3194–3212 | 窗口变为 key 时的那一帧也走事务 |
| `src/window.rs` 3342–3363 | 平时由显示链路驱动的帧不走事务 |

我们的图层可以自己设这个属性，wgpu 不会覆盖：`wgpu-hal` `src/metal/surface.rs` 61–108 的 `configure` 只设 opaque、设备、像素格式、`framebufferOnly`、EDR、drawable 数量和大小、`allowsNextDrawableTimeout`、`displaySyncEnabled`，从不碰 `presentsWithTransaction`；185 行在取 drawable 时读这个属性，`src/metal/mod.rs` 615–624 按它选择异步送显或 `commit` → `waitUntilScheduled` → `present`。所以每一帧都能临时决定走不走事务（原型的 `mirror` 模式就是跟着 `inLiveResize` 走）。

**不能用 `Instance::create_surface(window)`。** 它经 `raw-window-metal` 的 `Layer::from_ns_view` 取视图的图层，图层本身是 `CAMetalLayer` 时直接拿来用（`raw-window-metal` `src/lib.rs` 344–357）。`GPUIView` 的图层正是 GPUI 自己的 `CAMetalLayer`（`src/window.rs` 3272–3276 `make_backing_layer`），会和 GPUI 抢同一个图层。必须自己建图层，再用 `SurfaceTargetUnsafe::CoreAnimationLayer`（`wgpu` `src/api/surface.rs` 386–392）。

### 3.2 Windows（未实测）

| 位置 | 内容 |
|---|---|
| `gpui-pre-windows` `src/directx_renderer.rs` 1017 | `CreateTargetForHwnd(hwnd, true)`：GPUI 只建了顶层目标 |
| 同上 1298–1322 | 合成交换链 `DXGI_ALPHA_MODE_PREMULTIPLIED`，与背景设置无关 |
| 同上 350–357 | 非不透明背景时清屏颜色是 `[0; 4]` |
| 同上 251 | `Present(0, 0)`：不等垂直同步，由 DWM 在合成时取最新的缓冲 |
| `src/window.rs` 513–515 | 启用 DirectComposition 时窗口带 `WS_EX_NOREDIRECTIONBITMAP`，没有 GDI 重定向表面 |
| `src/window.rs` 916–940 | `Transparent` 时调用 `SetWindowCompositionAttribute`（accent 2） |
| `src/window.rs` 595–603 | `HasWindowHandle` 给出 HWND |
| `src/directx_renderer.rs` 1324–1350 | 设置 `GPUI_DISABLE_DIRECT_COMPOSITION` 后改用 `CreateSwapChainForHwnd` 和 `ALPHA_MODE_IGNORE`，方案 F 不可用 |

微软文档（`IDCompositionDevice::CreateTargetForHwnd`）：每个窗口在概念上有四层，自下而上是直接画到窗口的内容、一棵可选的 DirectComposition 树、子窗口、另一棵可选的 DirectComposition 树；`topmost` 为真的目标在第 4 层，为假的在第 2 层；每个窗口最多两个目标，一个顶层、一个非顶层，同一层已有目标时创建失败。所以**我们的非顶层目标正好在 GPUI 的下面**，而且不需要 GPUI 的设备。

wgpu 这边有两条路：

1. **自己建 DirectComposition 设备、非顶层目标和视觉对象**，用 `SurfaceTargetUnsafe::CompositionVisual`（`wgpu` `src/api/surface.rs` 394–400，只有 DX12 后端）。`wgpu-hal` `src/dx12/mod.rs` 1384–1395 用 `CreateSwapChainForComposition`，1462–1470 只调 `SetContent`，**不提交**，`Commit`、`SetOffsetX/Y`、裁剪都由胶水负责。支持的合成 alpha 模式包括 `Opaque`（`src/dx12/adapter.rs` 1297–1306）。
2. **`Dx12SwapchainKind::DxgiFromVisual`**（`wgpu-types` `src/backend.rs` 648–667）：wgpu 自己为 HWND 建一个非顶层目标（`wgpu-hal` `src/dx12/dcomp.rs` 98–121），视觉对象铺满整个窗口，不能设偏移。要用它就得像原型的 `VIEWPORT_POC_LAYER=window` 那样把视口画面拷到整窗交换链的对应位置，胶水最少，但每帧多一次整窗清屏和拷贝，macOS 上的同类实验同步反而更差（4.3 节）。

**同步：** GPUI 的设备和我们的设备各自 `Commit`，交换链各自 `Present`，DWM 在合成时各取最新内容，没有跨设备的原子提交。这和 macOS 平时的情形相同，预计同样会在布局动画时偶发错开一帧；拖动改变窗口大小时 GPUI 也没有事务机制，可能更明显。**需要在 Windows 上实测。**

### 3.3 Linux Wayland（未实测）

| 位置 | 内容 |
|---|---|
| `gpui-pre-linux` `src/linux/wayland/window.rs` 1686–1710 | `HasWindowHandle` 给出 GPUI 的 `wl_surface` 指针，`HasDisplayHandle` 给出 `wl_display` 指针 |
| `gpui-pre-linux` `Cargo.toml` 205–212 | `wayland-backend` 开了 `client_system`，即用系统 libwayland |
| `wayland-backend` `src/sys/client_impl/mod.rs` 271、99 | `Backend::from_foreign_display` 在同一个 `wl_display` 上另建事件队列；`ObjectId::from_ptr` 把 GPUI 的 `wl_surface` 包成我们这边的代理对象 |
| `src/linux/wayland/window.rs` 2289–2320 | 透明背景时不设不透明区域（`set_opaque_region(None)`），合成器会画下面的内容 |
| `gpui-pre-wgpu` `src/wgpu_renderer.rs` 415–428、1197–1205 | 透明时选 `PreMultiplied` 合成模式，每帧清成透明 |
| `src/linux/wayland/window.rs` 853–860 | GPUI 用分数缩放协议和 `wp_viewport`：子表面也要有自己的 `wp_viewport`，按物理像素渲染、按逻辑大小显示 |
| 全 crate | 没有绑定 `wl_subcompositor`，要在自己的事件队列上绑定 |

协议（`wayland-client` 自带的 `wayland.xml`，`wl_subsurface`、`wl_surface.commit`）：子表面创建时是**同步模式**；`set_position`、`place_above`/`place_below` 是**父表面的双缓冲状态**；同步模式下子表面的提交进入队列，在父表面下一次提交时与之一起原子生效。也就是说，只要我们在 GPUI 提交之前送显（原型里正是这样：我们在 GPUI 的 paint 阶段送显，GPUI 在之后的 `present` 才提交），**位置、层叠和内容都和 GPUI 的这一帧原子生效**，这是三个平台里唯一不用猜时序的。

风险：

- Vulkan 送显节流：Mesa 等驱动的 Wayland WSI 在 FIFO 模式下会等表面的帧回调或用 FIFO 协议节流；同步子表面的内容要等父表面提交才生效，如果送显和 GPUI 的提交在同一线程上互相等待，可能卡住。用 `Mailbox` 或 `Immediate`，或在没有新内容时不送显，需要实测。
- 父表面必须用带 alpha 的格式和预乘模式；GPUI 只在驱动支持时选 `PreMultiplied`（415–428），否则退回别的模式，需要在 NVIDIA 等驱动上确认。
- wgpu 实例要能在 Wayland 显示上建 Vulkan 表面（创建表面时传入 Wayland 的显示和窗口句柄）。

### 3.4 Linux X11

X11 的子窗口总是盖在父窗口自己画的内容上面（父窗口画不到子窗口的区域），合成管理器也是把整个顶层窗口连同子窗口作为一张图合成。没有“放在父窗口内容下面”的位置。另开一个顶层窗口放在后面，位置和层叠都由窗口管理器决定，无法可靠跟随。**不可行，继续用方案 A。**

## 4. macOS 原型

### 4.1 做法

代码在分支 `claude/underlay-feasibility-5c0830` 的 `apps/viewport-poc` 里，用 `VIEWPORT_POC_PRESENT=underlay` 切换到方案 F，其他开关见它的 README。和方案 A 共用同一个布局、同一个渲染器、同一套基准。

- **渲染器**：`mw-crystal-render` 新增 `Renderer::render_to`，把多重采样结果直接解析到调用方给的纹理（例如 drawable）；回读用的解析纹理改为第一次回读时才建；另外暴露 `instance()`、`adapter()`、`device()`、`queue()` 和 `COLOR_FORMAT`。渲染器仍然不知道 GPUI，也不含平台代码。
- **胶水**（`src/underlay.rs`，约 300 行，其中 unsafe 两处）：
  1. 从 `Window` 的 `HasWindowHandle` 取 `GPUIView`，再取它的父视图（内容视图）；
  2. 建一个承载图层的 `NSView`（layer-hosting），根图层是普通 `CALayer`（`geometryFlipped`，坐标和 GPUI 一致，左上角为原点、单位是点），背景色作为底色；`CAMetalLayer` 是它的子图层；
  3. `addSubview:positioned:NSWindowBelow relativeTo:GPUIView`，随内容视图自动缩放；
  4. `create_surface_unsafe(CoreAnimationLayer(..))` 建绘制目标，格式 `Bgra8UnormSrgb`、`Opaque`、`Fifo`；
  5. 每帧在 GPUI 的 paint 阶段：按视口的 `bounds` 设图层的 `frame`（在关闭隐式动画的事务里），尺寸变了就重新 `configure`；画面需要更新时取 drawable、`render_to`、`present`。也就是说**我们的送显和 GPUI 自己的送显在主线程的同一轮里**。
- **unsafe 的不变量**：`GPUIView` 由窗口持有、在窗口存续期间有效，我们在主线程上并且只是多持有一个引用；`CAMetalLayer` 由 `Underlay` 持有，字段顺序保证 `wgpu::Surface` 先于图层析构，wgpu-hal 自己也会持有一份。
- **布局**：窗口用 `WindowBackgroundAppearance::Transparent`；根视图背景改为透明；侧栏、右侧面板、底部面板各自画背景，视口区域什么都不画。
- **拾取不变**：仍是离屏画 ID 纹理再回读一小块（`viewport-rendering.md` 5.4 节）。基准里两种方案的拾取耗时在同一水平，相机未变时都是 1.2–1.3 ms。

### 4.2 检查方法

截图用 `screencapture`；两层是否同步用 `ffmpeg` 以 60 fps 录屏，逐帧用 `colorhold` 只保留某种颜色、`signalstats` 读最大饱和度，统计“出现这种颜色”的帧数：

- **几何是否对齐**：底色设为洋红（`VIEWPORT_POC_BACKDROP=debug`），GPUI 和下面的图层都不会出现洋红，**只有洞和图层错开时**才露出底色。用 `VIEWPORT_POC_STRESS` 每 0.7 秒收起或展开侧栏（400 ms 动画）或底部面板（瞬间），只录视口边缘移动的那一条。
- **内容是否同一帧**：被跟踪的原子画成纯绿，GPUI 在它的投影位置画一个刚好盖住它的不透明圆（`VIEWPORT_POC_MARKER=cover`），以 3 rad/s 旋转。两层是同一帧时绿色完全被盖住。
- **对照**：布局不动时洋红帧为 0/176；方案 A 下故意用当前相机而不是画面的相机放圆（`cover-stale`），每一帧都露出绿色（448/448，截图里绿原子在圆旁约 20 px），证明这个方法能测出一帧之差。
- 有一次录制恰好被屏幕上别的东西干扰（全部帧都“有绿”，但无损抓帧里找不到任何绿色像素），已作废重测；之后每次录制都同时存一张截图备查。

### 4.3 结果

**弹层和外观**（截图逐张检查）：

- 视口左上角的信息标签、跟踪原子的标记、浮动工具条、打开的弹出层、通知都正确画在 3D 画面上面；模态对话框的半透明遮罩把 3D 画面和 GPUI 面板一起压暗。
- 窗口阴影和圆角：方案 A、方案 F（有底色）、方案 F（无底色）三张截图的窗口角和阴影一模一样。GPUI 给透明窗口设 alpha 0.0001 的背景色（`src/window.rs` 1876）起了作用。
- 主题漏洞：没有底色（`VIEWPORT_POC_BACKDROP=0`）时，静止画面里也看不到任何透出桌面的地方，说明每个区域都画了自己的背景。深色主题下 3D 背景和底色都跟着变。
- 标题栏由 AppKit 绘制，不受影响。

**几何是否对齐**（洋红帧 / 录到的帧，每次 8 秒，约 11 次布局变化）：

| 情形 | 图层只盖视口（默认） | 图层盖住整个窗口 |
|---|---|---|
| 布局不动 | 0/176 | — |
| 侧栏动画，`mirror`（默认） | 0/481、6/462 | 85/481、102/481 |
| 侧栏动画，`async` | 0/481、8/479 | 93/481、99/481 |
| 侧栏动画，`transaction` | 133/481、105/476、123/467 | — |
| 底部面板瞬间切换，`mirror` | 0/481 | — |
| 底部面板瞬间切换，`transaction` | 0/481 | — |

逐帧看错开的那些帧：

- **`mirror`/`async`**：错帧是成段出现的（一段 15–21 帧，正好是一次动画），这一段里我们的图层已经移到新位置、GPUI 的洞还在旧位置，差一帧。图层的位置随主线程的 Core Animation 提交生效，几乎不等；GPUI 的像素等 GPU 完成后才异步上屏。GPUI 这一帧赶上合成器的截止时间，两者同时上屏；赶不上就晚一帧，而且动画期间节奏稳定，会整段都错开。
- **`transaction`**：我们的图层和内容一起随事务生效，事务要等我们的 GPU 工作完成，比 GPUI 的异步上屏晚，错帧多得多。
- **图层盖住整个窗口**（视口位置画在像素里，位置和内容都走 GPU 异步上屏）：我们每帧的 GPU 工作更重（按新尺寸重画、整窗清屏、拷贝），反而比 GPUI 晚一帧，更差。

结论：**外部图层没法和 GPUI 平时的异步送显原子提交**，只能尽量对齐。最好的组合是图层只盖视口加 `mirror`。把底色设为视口自己的背景色（原型的默认值）后，错开的那一帧只是视口边缘少几个像素的 3D 画面，同色，肉眼很难看出。

**内容是否同一帧**（绿色帧 / 录到的帧，1 万原子、1792 × 1600、3 rad/s）：

| 情形 | 结果 |
|---|---|
| 方案 A，标记用画面的相机 | 0/318、0/364 |
| 方案 A，标记用当前相机（对照） | 448/448 |
| 方案 F `mirror` | 0/481 |
| 方案 F `async` | 0/380 |
| 方案 F `transaction` | 0/481、0/337 |

匀速旋转时，3D 画面和 GPUI 标注始终是同一帧：我们在 GPUI 这一帧的 paint 里用同一个相机渲染并送显，两边的 GPU 工作都轻，同时上屏。

**拖动改变窗口大小**：这时 GPUI 走事务，`mirror` 也跟着走事务，按 3.1 节的机制两边应在同一个 Core Animation 事务里原子生效。**还没有实测**：需要真人拖动窗口边缘，本次测量时屏幕已锁定。

### 4.4 性能（初步，需复测）

环境：Apple M5，内建屏 60 Hz，窗口 1440 × 900，视口 1792 × 1600 物理像素，debug 构建（GPUI 和 wgpu 为 `opt-level = 3`），自动旋转，每档预热 1 秒、测 5 秒。

可信的一组（屏幕点亮、窗口在前台，`VIEWPORT_POC_PRESENT=underlay`，`mirror`）：

| 负载 | 显示帧率 | 编码提交 | 取 drawable + 送显（界面线程） | 提交 → GPU 完成 | 请求 → GPU 完成 | 本进程 CPU | WindowServer CPU |
|---|---|---|---|---|---|---|---|
| 1 千原子 | 60 | 0.18 | 0.04 | 2.77 | 3.81 | 10% | 18% |
| 1 万原子 | 60 | 0.18 | **7.46** | 5.96 | 14.45 | 10% | 17% |
| 静止 3 秒 | — | — | — | — | — | 0.2% | 2.9% |

时间单位 ms。随后跑了一组两种方案的完整对比，但跑的过程中屏幕先变暗、后休眠并锁定：方案 A 有 7 档只有 30 fps（同一份方案 A 代码 10-22 测得 60 fps，回读路径这次没有改动），方案 F 最后两档窗口失去焦点。这组数据不作结论，只摘出两点：

- 方案 F 的界面线程开销是双峰的：同一档负载，取 drawable 有时 0.03 ms，有时 7–20 ms；
- 两种方案静止时本进程 CPU 都不超过 0.5%，按需渲染的性质保持不变。

**取 drawable 阻塞**是原型发现的主要问题：`CAMetalLayer` 最多 3 个 drawable，一个在屏上、一个排队、一个在画；我们每帧的 GPU 工作比 GPUI 重，drawable 归还得晚，`nextDrawable` 就在主线程上等。量产实现要把取 drawable、送显放到渲染线程（wgpu 的设备和绘制目标都能跨线程用），只在拖动改变窗口大小时回到主线程走事务。代价是平时的送显不再和 GPUI 这一帧在同一轮里，内容同步要重新测。

**收益估计**（按 `viewport-rendering.md` 8.1、8.2 节的方案 A 数据）：方案 F 省掉每帧的回读等待、内存拷贝和 GPUI 上传（1792 × 1600 下界面线程约 1.5–3 ms），画面在 GPU 完成后的下一次垂直同步上屏，而方案 A 要等下一次 GPUI 帧才画出来，延迟少约一帧。4K 或 120 Hz 下差距会更大。

### 4.5 顺带发现

- **方案 A 的标注比画面快 1–2 帧**：`mw-viewport` 的 `Viewport::project` 用的是当前相机，而屏幕上的画面是 1–2 帧之前的相机渲染的。对照实验里，3 rad/s 旋转时标记和原子差约 20 px。正常拖动慢得多，但测量标签在旋转时仍会轻微“漂”。改法：投影时用屏幕上那一帧的相机（`mw-viewport` 记住每帧的相机即可）。
- **GPUI Kit 根视图的背景**：要留洞就必须清掉根视图的背景，这会影响整个外壳的布局约定（3.1 节）。

## 5. 工作量估计（量产）

| 部分 | 内容 | 估计 |
|---|---|---|
| 公共 | `mw-viewport` 里抽出呈现方式（回读 / 底层），出错时自动退回方案 A；外壳改为各区域自己画背景、根视图透明；截图巡览和界面测试固定用方案 A；标注改用屏幕上那一帧的相机 | 2–3 天 |
| macOS | 原型胶水整理；渲染线程送显，解决 drawable 阻塞并重测同步；跨显示器（`contentsScale`）、遮挡、窗口变为 key 时的事务帧；拖动改变窗口大小的实测 | 3–4 天 |
| Windows | windows-rs 的 DirectComposition 胶水（非顶层目标、视觉对象、偏移、裁剪、提交）；强制 wgpu 用 DX12；DPI；需要 Windows 机器实测同步 | 3–5 天 |
| Wayland | 同一 `wl_display` 上的事件队列，绑定 `wl_subcompositor`，子表面 `place_below`，`wp_viewport` 处理分数缩放，送显模式；在 GNOME、KDE、Sway 和 NVIDIA 上实测 | 4–6 天 |
| X11 | 不做，用方案 A | 0 |

合计约三周，另有每次升级 GPUI Kit 的复核（第 8 节）。

## 6. 仍未解决的风险

| 风险 | 平台 | 说明 |
|---|---|---|
| 与 GPUI 的异步送显无法原子提交 | macOS、Windows | GPUI 驱动的布局动画中约 1.5% 的帧错开一帧（macOS 实测）；只能用同色底色掩盖。Windows 预计类似，未测 |
| 拖动改变窗口大小 | 全部 | macOS 理论上同步，未实测；Windows 没有事务机制，可能错帧；Wayland 理论上同步 |
| 主线程取 drawable 阻塞 | macOS | 实测 7–20 ms；要渲染线程，渲染线程又会削弱和 GPUI 同轮送显的同步 |
| 透明窗口的布局约定 | 全部 | 根视图透明，每个区域自己画背景；以后任何组件在视口上方留了透明区域都会透出底色。半透明图元重叠处 alpha 偏大（3.1 节），影响很小 |
| 截图和测试拍不到 3D 画面 | 全部 | GPUI 的 `render_to_image` 只有 GPUI 这一层，测试和截图巡览必须用方案 A。系统截图（`screencapture`）拍得到两层 |
| Wayland 送显节流、预乘 alpha 支持 | Wayland | 同步子表面和 FIFO 可能互相等待；部分驱动不支持预乘合成模式 |
| 显卡选择 | 双显卡 | wgpu 要和 GPUI 选同一块 GPU（`viewport-rendering.md` 5.2 节） |
| 依赖 GPUI 的内部结构 | 全部 | macOS 依赖 `GPUIView` 是内容视图的子视图、GPUI 只在 `displayLayer:` 里走事务；Windows 依赖 GPUI 只用顶层目标；Wayland 依赖窗口句柄是 GPUI 的 `wl_surface` |

## 7. 建议

**现在不采用方案 F，保持方案 A。** 理由：

1. 方案 A 在当前目标下够用：1 万原子、全分辨率 60 fps，界面线程每帧 1–3 ms（`viewport-rendering.md` 8.1、8.2 节）。
2. 方案 F 在 macOS 上可行，但收益（每帧 1.5–3 ms、少一帧延迟）要用透明窗口的布局约定、偶发错帧和一个渲染线程来换；Windows、Linux 的同步还没测。
3. macOS 有更干净的路：上游 #61291 合并后，GPUI 的 `surface()` 能直接合成 BGRA 的 IOSurface，在 GPUI 自己的帧里绘制，天然同步，窗口也不必透明（`viewport-rendering.md` 6.1 节）。

**什么时候启动**：只有当实测证明方案 A 及其优化（交互时降分辨率、保守深度）在目标平台上不够用——例如 4K 或 120 Hz 下界面线程每帧超过 4 ms、或在 Linux、Windows 上掉帧——并且方案 C 进不了上游时。按平台的顺序：

1. **Linux Wayland 优先**：同步子表面给出真正的原子提交，三个平台里同步条件最好；Linux 上 GPUI 本来就用 wgpu。
2. **Windows 其次**：DirectComposition 的分层是系统支持的，胶水简单，但同步要先在实机上量。
3. **macOS 最后**：先等 #61291；等不到时再用本原型的做法，并先解决 drawable 阻塞。

启动前先补完这个原型的三项测量：拖动改变窗口大小时的同步、屏幕常亮条件下两种方案的完整基准对比、渲染线程送显后的同步和界面线程开销。

## 8. 复现

先切到分支 `claude/underlay-feasibility-5c0830`：

```bash
VIEWPORT_POC_PRESENT=underlay cargo run -p viewport-poc
VIEWPORT_POC_PRESENT=underlay VIEWPORT_POC_BENCH=1 cargo run -p viewport-poc
```

同步测试的做法见 4.2 节，开关见 `apps/viewport-poc/README.md`。录屏要求终端有屏幕录制权限，窗口保持在前台，屏幕不能变暗或锁定。

升级 GPUI Kit 时，除 `viewport-rendering.md` 附录的清单外，再核对：

1. macOS：`GPUIView` 是否仍是 `contentView` 的子视图、是否仍不重写 `hitTest:`；`presents_with_transaction` 是否仍只在 `displayLayer:` 和变为 key 时打开；透明时清屏 alpha 是否仍为 0。
2. `gpui-component` 的根插件是否仍给根元素设背景，`Root` 自己的样式是否仍在最后应用。
3. Windows：是否仍只建顶层目标、合成交换链是否仍是预乘 alpha。
4. Wayland：窗口句柄是否仍是 `wl_surface`，是否仍用系统 libwayland。
5. wgpu：Metal 的 `configure` 是否仍不碰 `presentsWithTransaction`；`CoreAnimationLayer`、`CompositionVisual` 是否仍在。
