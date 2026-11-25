# Linux 掌机兼容边界

## 逻辑

Rudroid 生产目标是一层明确的用户态 ABI，不是“ARM64 芯片就能跑”：

**ELF64 LE AArch64 + GNU glibc**，核心 kit 直接 VERNEED 上限 **GLIBC 2.28**
（部署门禁静态审计）。其上每一层（内核页大小、SDL/EGL/GPU、guest Bionic、
音画输入、Unity 退出）各自有“当前怎么处理 / 什么仍未证明”的边界；下层绿
不能代替上层验收。

实机范围与局限归 [ACCEPTANCE](ACCEPTANCE.md)；装载/交换等踩坑归
[DEVELOPMENT · 已知适配陷阱](../../DEVELOPMENT.md#已知适配陷阱)；
构建与 `linux_abi.py` 命令归
[BUILD-DOCKER · 生产部署](../../BUILD-DOCKER.md#生产部署顺序)。

已测设备（证据，不是通用保证）：

- 2026-10-17 MiniLoong / RK3566 / Mali-G52 / LoongOS 1.4.0.36 / GLIBC 2.38 — 五标题有界路径。
- 2026-10-19 TrimUI / A523 / Mali-G57 / ~1 GiB / Buildroot 2022.05 / GLIBC 2.33 — SDL 2.32.6 KMSDRM 1280×720；系统无 Pulse/Wayland backend 时用局部 SDL+Vulkan loader（客户端 GLIBC **2.29**，不抬高核心 kit 声明）。细节与未修 Vulkan 画面见验收页。

不能据此声称 GLIBC 2.28 固件、R36S 或未登记板型已通过。

## 分层边界

| 层 | 当前处理 | 未支持或仍需验证 |
|---|---|---|
| CPU / 用户态 | ELF64 LE AArch64，GNU target；固定入口不传 `target-cpu=native` | ARM32、musl、其他架构；芯片 64 位 ≠ 固件有 64 位库 |
| GNU libc | 生产库存直接 VERNEED ≤ **2.28**（含 weak statx），部署逐次审计 | 2.17–2.27 不能沿用此产物；降基线要兼容 sysroot 另构建并验收 |
| 内核 / 页大小 | 读 native page size；拒绝页级 W/X 冲突；syscall 失败按实系统处理 | 多旧内核与 16/64 KiB 页未实测 |
| SDL / EGL / GLES | SONAME `libSDL2-2.0.so.0`；ES 3.2→2.0；共享窗口/context | 沿目标 DT_NEEDED 核真实驱动链；Docker Mesa ≠ 硬件性能；主窗口交换须走 SDL（KMSDRM 含 page flip） |
| ELF / guest ABI | Bionic 版本化导入、RELR/RELA/APS2、FILE/线程 facade、init/fini | 非 Android 系统 linker；TLS/自动依赖/任意路径 dlopen 等见 [MIGRATION-MAP](MIGRATION-MAP.md#剩余差异与未支持范围) |
| 音频 / 输入 | 共享 AudioOutput/InputSource（现实现为 SDL），FMOD 等复用 | 听感、全设备、热插拔、全动作另验；NDK sensor 无设备；media fallback 不解码视频 |
| Unity / 退出 | 共用 player 生命周期；nativeDone 前停托管、等回调；音频 stop/join 后释放 | 成功 capability/guest/SDL 保持进程有效期；有界 Mono 见 [UNITY-MONO](UNITY-MONO.md) |

补充：

- Android `free@LIBC / libc.so` 是 Bionic ABI，不靠升级掌机 glibc 或裸名 `dlsym` 解决；layout/version 在边界转换。
- barrier/spin 已实现 ≠ process-shared / Android TCB / 完整 NPTL。
- 编译器 target 基线 ≠ 链接后 ELF 基线；以 VERNEED/DT_NEEDED 与目标实文件为准。AOSP 参照：[libc.map.txt](https://android.googlesource.com/platform/bionic/+/android-15.0.0_r1/libc/libc.map.txt)。

图形默认与 Vulkan 准入（五标题 GLES 75%；丝之歌/吸血鬼手动 Vulkan 门禁）见
[RUNTIME-POLICY](RUNTIME-POLICY.md#可选-vulkan-后端) 与 [ACCEPTANCE](ACCEPTANCE.md)。
TrimUI 私有 Weston、A523 丝之歌全尺寸合成等策略也落在那两处，不在本文展开。

## 交付时查什么

生产部署顺序（整套重建、成员、test-marker、GLIBC 审计）只走 BUILD-DOCKER。
`cargo test` 可能留下测试 feature 的 cdylib，不能只换单个旧库交付。

`linux_abi.py` 静态读 PT_INTERP、DT_NEEDED、GNU 版本表，不跑 ELF/ldd；
deploy-select 在改部署目录前拒绝超限/错架构/缺表/测试成员。报告只覆盖所给文件的
**直接**需求，仍须沿 SDL/EGL/GLES/libz/libgcc/驱动链核验。`--max-glibc 2.17`
只拒绝超限，不降级二进制。缺依赖优先游戏目录局部库与 `LD_LIBRARY_PATH`/shim；
不得借兼容之名换系统 libc/GPU 或改固定构建镜像。

syscall、显示/音频/输入、资源上限与退出均在目标固件测定。新设备走
[移植预检](HANDHELD-PORTING-WORKFLOW.md)，不复用他机一句“已通过”。
