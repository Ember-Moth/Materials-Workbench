# Materials Workbench

用 Rust 编写的现代化、开源、跨平台材料科学桌面工作台。第一阶段做晶体学：查看、测量、编辑晶体结构，再导出给下游计算；之后逐步加入 XRD、相图和显微组织分析。

界面基于 [GPUI Kit](https://gpui-kit.com)，3D 视图用 wgpu 离屏渲染。所有能力都可以被 AI 调用：外部 AI Agent 通过 MCP 操作工作台，应用内置的助手用自然语言分析数据。

## 状态

v0.1 开发中：界面框架已经按设计稿搭好，3D 视图可以旋转缩放，数据来自内置示例；文件导入、对称性计算和编辑功能还是模拟，正按里程碑逐步实现。

## 路线图

| 版本 | 主题 |
|---|---|
| v0.1 | 晶体学 Alpha：CIF 导入、对称性、渲染与拾取、基础编辑、导出、项目文件 |
| v0.2 | 对称编辑：特殊位置约束下的移动与添加、对称性检测与对称化、晶胞变换；实验性只读 MCP Server |
| v0.3 | 晶体学完善：晶面、显示范围、多面体；Python、命令行与完整的 MCP 接口；首个公开发布 |
| v0.4 | 衍射与内置助手：粉末 XRD 模拟、实验谱导入、寻峰与物相比对；内置 AI 助手第一版 |
| v0.5 | 热力学：二元体系 CALPHAD 计算与相图 |
| v0.6 | 显微组织：金相 / SEM 图像分割与统计 |
| v1.0 | 稳定版：项目格式、扩展接口、安装包与文档 |
| v1.x | 实验室数据：仪器数据自动导入与批处理、仪器适配插件框架 |

每个版本的功能、算法和验收标准见[产品路线图](docs/roadmap.md)。

## 运行

需要 Rust 1.92 或更新；macOS 上需要 Xcode。

```bash
cargo run --bin materials-workbench
```

## 文档

- [产品路线图](docs/roadmap.md)：定位、竞品、各版本的功能、算法与验收标准
- [架构](docs/architecture.md)：模块划分、依赖方向、模拟清单与当前里程碑
- [开发指南](docs/development.md)：环境、命令、代码约定、测试与提交
- 设计稿：[界面外壳](docs/design/ui-shell.md) · [晶体学数据模型](docs/design/crystallography-model.md) · [3D 视口](docs/design/viewport-rendering.md) · [AI 集成](docs/design/ai-integration.md)
