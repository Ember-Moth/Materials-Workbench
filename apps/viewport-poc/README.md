# viewport-poc

验证 [3D 视口技术选型](../../docs/design/viewport-rendering.md) 中的方案 A：wgpu 离屏渲染 → 回读到内存 → GPUI `Window::paint_image`。

渲染部分在 [`mw-crystal-render`](../../crates/mw-crystal-render)，只依赖 wgpu；这个应用只负责把回读的图像放进 GPUI 布局，并统计耗时。

## 运行

```bash
cargo run -p viewport-poc
```

- 拖动旋转，滚轮缩放。
- 右侧面板可以切换原子数（1 千 / 1 万 / 10 万）、渲染缩放（100% / 50%）和自动旋转，并显示各阶段耗时。
- 视口底部的浮动工具条用来检查 GPUI 弹出层（Tooltip、Popover）能否盖在回读图像上。

只想看渲染器的输出、不开窗口时：

```bash
cargo run -p mw-crystal-render --example snapshot -- snapshot.png 1000
```

## 基准模式

```bash
VIEWPORT_POC_BENCH=1 cargo run -p viewport-poc
```

自动旋转并依次跑完 3 档原子数 × 2 档渲染缩放，每档预热 1 秒、测量 5 秒，打印一行结果后自动退出。窗口需要保持在前台，否则系统可能降低刷新频率。

也可以指定要跑的档位和顺序（原子数:渲染缩放百分比）：

```bash
VIEWPORT_POC_BENCH="100000:50,10000:100" cargo run -p viewport-poc
```

输出字段：

| 字段 | 含义 |
|---|---|
| `render` | 实际渲染尺寸（物理像素 × 渲染缩放） |
| `frame_mb` | 每帧回读的数据量 |
| `presented_fps` | 每秒画到屏幕上的新图像数 |
| `finished_fps` | 每秒完成回读的帧数（包括来不及显示就被替换的帧） |
| `encode_ms` | 编码并提交 GPU 命令 |
| `gpu_wait_ms` | 从提交到像素可读（GPU 渲染 + 拷贝到暂存缓冲） |
| `copy_ms` | 把映射的数据按行拷进紧凑缓冲 |
| `paint_ms` | `paint_image` 耗时，包含上传到 GPUI 精灵图集 |
| `request_to_image_ms` | 从相机变化到新图像就绪（尚未绘制） |
| `window_lost_focus` | 测量期间窗口是否失去过焦点；为 `true` 时帧率可能被系统限制，结果不可信 |
