# 分形文档协议

从[根 AGENTS](AGENTS.md)进入[workspace](crates/AGENTS.md)，再读目标模块。
文档入口见[索引](docs/README.md)，只加载与任务相关的路径。

## 维护范围

维护的 Rust、C/头文件/汇编、Python、Shell 源码及测试保留三行头注释：
`INPUT:`、`OUTPUT:` 后两个空格，`POS:` 后四个空格。

```rust
//! INPUT:  HostApi、共享 JNI VM、Unity profile Spec
//! OUTPUT:  prepare/bootstrap/run 生命周期
//! POS:    在版本 profile 之间复用 Unity 玩家启动与停止流程
```

沿用语言的注释风格，保留 shebang、编码声明、许可证与来源；头注释放在
import/执行语句之前。先阅读实现和公开接口，不从文件名猜职责。
不为普通文档、配置、图片或二进制添加源码头注释；排除第三方原样文件、
生成文件、隐藏工具目录及忽略的 target/dist/缓存/游戏输入。

## 目录与内容分工

每个 crate 一个英文 AGENTS，使用 Role / Design / Constraints / Contents，
只保留职责、调用关系、所有权/ABI 约束和真实文件导航。中间层或 src/tests
不重复建文档；fixture 的 ABI/生命周期证据随实现保留。

命令归 BUILD-DOCKER，开发与踩坑归 DEVELOPMENT，能力归 MIGRATION-MAP，
配置归 RUNTIME-POLICY，交付归 ACCEPTANCE，性能取舍归 PERFORMANCE-AUDIT。
删掉过时日报和重复说明，只留下仍会影响实现的失败原因及复现前提。

## 变更检查

- 接口/职责变更同步源码头和模块 AGENTS；增删/迁移同步上级导航。
- 移动或删除文档同步全部本地链接、锚点；检查围栏、真实路径和失效条目。
- Rust/native/build.rs 等输入会改变 kit 指纹；构建期间不得修改这些输入。
  纯 Markdown 整理不重新编译或部署。交付仍遵守[构建规则](BUILD-DOCKER.md)。
