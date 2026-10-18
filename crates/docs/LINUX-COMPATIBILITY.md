# Linux 掌机兼容边界

当前构建与运行目标为 **ELF64 little endian AArch64 + GNU glibc**。
2026-10-17 在 MiniLoong/RK3566/Mali-G52/LoongOS 1.4.0.36、GLIBC 2.38 完成五标题
有边界的[实机验收](ACCEPTANCE.md)；不能据此声称已在 GLIBC 2.28、R36S 或所有固件运行。
必要的 ABI/装载陷阱见[开发规范](../../DEVELOPMENT.md#已知适配陷阱)。

2026-10-19 另在 TrimUI 的 Allwinner A523 / Mali-G57 / 986008 KiB RAM /
Buildroot 2022.05、GLIBC 2.33 上检查。设备树与真实库优先于 PortMaster 中旧的
A133plus 名称：SDL 2.32.6 使用 KMSDRM，原生输出 1280×720。
主窗口须由 SDL 完成交换及 page flip，不能只调用原生 `eglSwapBuffers`。
系统 SDL 未编译 PulseAudio backend，启动器沿用 ALSA 的 Pulse 插件连接现有声卡服务。
该驱动能创建 Vulkan 1.2 实例并枚举 GPU，但缺当前 Rudroid 所需的 Wayland 呈现会话；
Vulkan 不可选，保留 GLES。具体标题、内存及验收边界见[验收](ACCEPTANCE.md)。

## 系统要求

| 层 | 当前处理 | 未支持或仍需验证 |
|---|---|---|
| CPU / 用户态 | ELF64 LE AArch64，GNU target；固定入口不传 target-cpu=native | ARM32、musl、其他架构；64 位芯片不代表固件提供 64 位库。 |
| GNU libc | 当前生产库存直接 VERNEED 最高 **GLIBC_2.28**，含 weak statx，部署门禁逐次静态审计 | 2.17–2.27 不能沿用此产物；降低真实基线需兼容 sysroot 的独立构建及验收。 |
| 内核 / 页大小 | 映射器读取 native page size、拒绝页级 W/X 冲突；按实际系统处理 syscall 失败 | 多种旧内核与 16/64 KiB 页设备未实测；4 KiB 链接布局不能直接保证大页可运行。 |
| SDL / EGL / GLES | SDL SONAME `libSDL2-2.0.so.0`；ES context 3.2→2.0 尝试，共享窗口/context 与几何 | 真实 SDL、EGL、libgcc、GPU 驱动及传递依赖须沿目标 DT_NEEDED 核验。Docker Mesa 不证明硬件 GPU 性能。 |
| ELF / guest ABI | Bionic 版本化导入、RELR/RELA/APS2、FILE/线程/同步 facade、初始化与析构 | 不是 Android 系统 linker；TLS、自动依赖发现、任意 guest 文件 dlopen 等见[能力边界](MIGRATION-MAP.md#剩余差异与未支持范围)。 |
| 音频 / 输入 | 共享 AudioOutput/InputSource，当前实现为 SDL，FMOD 等复用；真实 PCM、轴/事件与退出有专项 | 扬声器品质、全部设备、物理拔插和标题全部动作需独立验收。NDK sensor 明确无设备，媒体 fallback 不解码视频。 |
| Unity / 退出 | 共用 player 生命周期；nativeDone 前停止托管工作，等待在途回调，音频 stop/join 后释放 | 成功 capability、guest 代码和 SDL session 保持进程有效期；不在有线程/函数指针时 dlclose。五 build 以外、Mono 与全流程另验收。 |

Android 的 `free@LIBC / libc.so` 是 Bionic ABI 要求，不靠升级掌机 glibc 或按裸名字
查宿主解决；layout/version 不同须在边界转换。barrier/spin 已实现，但 process-shared
同步、Android 私有 TCB 和完整 NPTL 兼容不因此成立。

Rust target 的编译器支持基线不等于链接后 ELF 的实际基线。以 VERNEED/DT_NEEDED
和目标真实文件为准；weak binding 不会自动消除版本节点，本项目采用保守上限。
固定 ABI 依据见 [Android libc 版本表](https://android.googlesource.com/platform/bionic/+/android-15.0.0_r1/libc/libc.map.txt)。

## 交付与固件检查

完整构建、成员选择、manifest、test-marker 与 GLIBC 审计只按[生产部署流程](../../BUILD-DOCKER.md#生产部署顺序)执行。
`cargo test` 可能留下测试 feature 的 cdylib，部署前必须纯生产整套重建；不能只换单个旧库。

```sh
python3 crates/scripts/linux_abi.py --dir /path/to/kit --max-glibc 2.28 --output /path/to/abi-report.json
python3 crates/scripts/linux_abi.py --file /path/to/target/libSDL2-2.0.so.0 --max-glibc 2.28
```

从仓库根执行，路径替换为实际文件。工具静态读取 PT_INTERP、DT_NEEDED、GNU
版本/导入表，不运行 ELF/ldd。deploy-select 在修改部署目录前拒绝超限、错误架构、
缺审计表或测试成员；原目录保留。Mach-O 流程独立，不等于 Linux 支持。

`--max-glibc 2.17` 只检查并拒绝当前超限产物，不降级二进制。若固件缺所需依赖，
优先游戏目录局部部署与 LD_LIBRARY_PATH/shim；不得以兼容修复名义替换系统 libc/GPU
或自动更换固定镜像、安装工具链。镜像约束见[构建指南](../../BUILD-DOCKER.md)。

报告只覆盖所给文件的直接需求，仍须沿 SDL/EGL/GLES/libz/libgcc/驱动的实际依赖链
核验一致性。syscall 可用性、显示/音频/输入、资源上限与游戏退出均在目标固件测定。
新设备执行[移植预检](HANDHELD-PORTING-WORKFLOW.md)，不复用其他设备的一句“已通过”。
