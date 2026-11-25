# Bionic API 完整性与测试边界

## 逻辑

`android_bionic` 在 Linux ARM64 GNU 上回答 guest 的 libc/libm/liblog 导入。
它不是完整 Android 15 Bionic。判断“适配到什么程度”分三层，不能互相代替：

1. **标题能跑** — 实机/游戏验收，不在本文分母内。
2. **符号按正确库名与版本解析** — 对照独立 AOSP 导出表的覆盖门禁；本文主责。
3. **已实现入口的行为与 ABI** — 单元/guest/conformance 证据；符号命中 ≠ 语义覆盖。

JNI、Java Framework、libandroid、EGL/Vulkan、ELF/libdl 由其他能力拥有，
不计入本节分母。实现与注册表见 [bionic AGENTS](../capability/android/bionic/AGENTS.md)；
跑覆盖报告与严格门禁的命令见
[BUILD-DOCKER · Bionic API 完整性](../../BUILD-DOCKER.md#bionic-api-完整性)。

## 分母从哪来

基准固定为 AOSP `android-15.0.0_r1`、ARM64、API 35。三个原始 export map
（[来源与 SHA256](../capability/android/bionic/tests/fixtures/android-15/README.md)）
是独立参照，**不从** Rudroid 符号表生成。按 Soong NDK 模式过滤后，分母含函数与
数据符号；排除私有/platform 版本、其他架构、未来 API、APEX/LLNDK/system-only。

## 覆盖怎么读

`android_15_coverage_report` 用生产 resolver 逐项查询，写出
`crates/target/docker-linux-arm64/target/bionic-completeness/android-15-arm64.tsv`。
当前计数以该 TSV 为准，文档不冻存数字。

| 状态 | 含义 |
|---|---|
| `resolved` | 正确库名 + 版本节点命中 |
| `bare-only-or-wrong-version` | 仅裸名或错误版本能命中；不算版本化通过 |
| `missing` | 未实现，或不能安全转交 GNU 的 Android 专用服务 |
| （额外列） | Rudroid 兼容别名等；不扩大 AOSP 分母 |

解读要点：

- 常规版本一致性只核对**已有** libc/libm 导出，并拒绝外来库名、私有/GNU/未知版本、半空 library/version。
- liblog 只接受无版本查询；已有入口不能记成版本化 ABI 通过。
- libm `ldexp` 是兼容别名（AOSP 归属 libc），不增加分母。
- `full_android_15_abi_completeness` 要求基准全部 `resolved`；默认 ignored。常规通过或 ignored **不等于** 完整 Android 适配；即使将来全绿，仍只是符号/版本门禁。
- 缺口含未实现 API 与不可转交的服务；不能靠任意 `dlsym` 回退消掉。常见缺口类：`accept4`、`sigqueue`、`sem_open`、`epoll_create1`、`posix_spawn`（以 TSV 为准）。
- 常规离线测试不下载 AOSP、不装 NDK、不读游戏输入。

## 行为证据止于行为族

下列是已验证的行为族，细节在对应测试与 guest fixture；**不是**逐 API 全路径覆盖率。

| 范围 | 证据落点 |
|---|---|
| allocator、errno、FORTIFY、边界字符串 | Bionic 单元测试；越界 SIGABRT / 保护页 / canary |
| FILE、变参、64 位偏移 | `legacy_libc`、`stdio_cost`、guest stdio；guest 不见 GNU FILE |
| 同步与线程属性 | 单元 + guest pthread/services |
| locale / UTF-8 / 宽字符 | conformance 与 locale 隔离测试 |
| 信号、网络、POSIX/终端 | `conformance` 与 guest canary；无外网 DNS |
| ELF 调用、析构、jump、libm | loader `bionic_guest`（版本化 AArch64 C guest） |

命名信号量、process-shared 同步、Android TCB/TLS、完整 property area、全部
sysconf selector，都不能由“同名符号存在”推定兼容。内核/驱动、目标固件、真实游戏
与长期运行须单独验收；Docker ARM64 GNU ≠ 所有 Android 版本或所有掌机。
