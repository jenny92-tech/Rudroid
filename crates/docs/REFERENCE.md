# C++ 行为参考规则

Bogodroid 是独立的只读参考仓库；Rudroid 编译和测试只使用自己的 workspace、
配置样例与 JNI fixture。当前五标题已完成本机基础交付，见[验收](ACCEPTANCE.md)。

1. 阅读 C++ 的 `projects/unityloader/`、`loader/`、`platform/`、`javastubs/`、
   `libjnivm/` 和 `thunks/` 对照行为；新实现放在 Rudroid。修改参考树需要明确任务授权。
2. Host Config 保持已有 schema 与行为兼容，容忍未知键。Reply Catalog 仍为独立、
   同构建二进制，不能合并为 TOML。具体消费者与差异查[能力表](MIGRATION-MAP.md)。
3. 转录 JNI 签名、engine Spec、ABI、接口名和固定回复时精确核对并测试。
   有意修正宿主策略时记录理由与执行证据，例如 Android 设备 manufacturer 默认
   `Rudroid` 避免误选 Linux 手柄布局；不能用“原样复制”掩盖上下文不匹配。
4. 固定地址、结构布局和托管调用必须有精确 build/ABI 门控。相邻 Unity 版本、
   同名方法或 C++ 中曾经存在的偏移都不足以证明可用。
5. 分别验证解析、运行消费者与设备效果。C 的成功退出不能证明 Rust 更深的
   nativeDone/join 清理正确；同一配置也不能证明同画质、内存或帧率一致。
6. 按实际触达面推进，持续如实记录未支持项。五标题短程通过不等于所有 API、
   全玩法、长期稳定性或所有 unsafe 已被证明绝对安全。

职责与术语见[开发规范](../../DEVELOPMENT.md)和[CONTEXT](../../CONTEXT.md)。
