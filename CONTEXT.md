# Rudroid 术语

Rudroid 是独立的 ARM64 Android guest 兼容运行时。Rust workspace 在 `crates/`，
按 android / linux / unity 组织，通过薄 loader 动态加载按需部署的能力。
当前交付范围见[验收](crates/docs/ACCEPTANCE.md)，模块位置见[workspace 导航](crates/AGENTS.md)。

| 术语 | 含义与边界 |
|---|---|
| Reference Implementation | 独立 Bogodroid 仓库中的 C++ 行为参考；供阅读和对照，不是 Rust 构建依赖，新功能落在 Rust。 |
| Parity | 配置、装载、JNI、音画、输入、生命周期与插件的可观察行为对齐目标；不等于全部 API 已完成或所有游戏已验收。明确记录有意差异和未支持项。 |
| Core Service | 某项数据与行为的权威来源；一个会话中的多个兼容外观共享状态及资源所有权。 |
| Compatibility Facade | 向 guest 暴露的 Android/JNI/NDK 等接口；不同接口不各建一套服务或缓存。 |
| Capability | 可独立部署、带 descriptor 的动态库，按 android/linux/unity 分层；能力间不直接链接。 |
| Android Capability | ELF、Bionic、JNI、framework、资源和 Android addon 等 guest 接口。 |
| Linux Capability | 宿主后端，例如 SDL 音画输入、日志和已解出的资源包视图。 |
| Unity Capability | 引擎接入、通用 IL2CPP 机制、版本 profile 和游戏补丁。 |
| Android Addon | GMS、Play Core、FMOD、Burst、helpers、XLua 等库级适配；保留既有 `sdk_*` id。 |
| Project | `unity_loader` 薄入口：读取配置、验证并装配能力和共享服务。 |
| Title | 具体游戏适配面；当前五标题与版本分别记录，不能从一个标题推断全部游戏支持。 |
| Host Config | 与现有启动器兼容的 TOML，例如 `[paths]`、`[package]`、`[plugins]`；字段存在不代表运行消费者已实现。 |
| Reply Catalog | 类/方法/签名对应固定或工厂返回值的同构建二进制；独立于 Host Config，不提供真实在线服务。 |
| Build-once Kit | 全 workspace 同一次身份的产物库存。构建身份由源码、目标、features 和 codegen 参数决定，loader 拒绝混用。 |
| Deploy Selection | 按标题/配置从完整 kit 拷贝成员子集；不是每个游戏静态重编一个引擎。 |
| Dynamic Link | loader 通过 Host API、共享 VM 和动态符号与能力协作；跨 cdylib 的 common static 不共享。 |
| Usage Gate | 可以按实际触达面排实现顺序；未触达能力仍需如实列缺口，不能用单标题通过证明完整兼容。 |

能力 id、Docker 容器名、Cargo package 与 ABI 导出名属于现有契约，
修改时同步全部消费者。具体规范见[开发指南](DEVELOPMENT.md)与[参考规则](crates/docs/REFERENCE.md)。
