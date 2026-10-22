# Materials Workbench

用 Rust 编写的现代化、开源、跨平台材料科学桌面工作台。第一阶段做晶体学：查看、测量、编辑晶体结构，再导出给下游计算；之后逐步加入 XRD、相图和显微组织分析。

界面基于 [GPUI Kit](https://gpui-kit.com)，3D 视图用 wgpu 离屏渲染。

## 状态

v0.1 开发中：界面框架已经按设计稿搭好，3D 视图可以旋转缩放，数据来自内置示例；文件导入、对称性计算和编辑功能还是模拟，正按[路线图](docs/architecture.md#10-路线图)逐步实现。

## 运行

需要 Rust 1.92 或更新；macOS 上需要 Xcode。

```bash
cargo run --bin materials-workbench
```

## 文档

- [架构](docs/architecture.md)：模块划分、依赖方向、模拟清单与路线图
- [开发指南](docs/development.md)：环境、命令、代码约定、测试与提交
- 设计稿：[界面外壳](docs/design/ui-shell.md) · [晶体学数据模型](docs/design/crystallography-model.md) · [3D 视口](docs/design/viewport-rendering.md)
