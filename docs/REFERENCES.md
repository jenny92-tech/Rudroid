# 参考项目与采用边界

本页保留固定来源、可借鉴机制及本地落点。外部项目的实现或性能数字不作为
Rudroid 的硬件、玩法或性能验收，第三方许可须按所用快照分别核对。

## nxnative

固定快照 [9579030](https://github.com/seedds/nxnative/tree/9579030e5b1f0443c12b7376559560915b6d4c4b) 的
[shader_split](https://github.com/seedds/nxnative/blob/9579030e5b1f0443c12b7376559560915b6d4c4b/dm/gfx/tools/shader_split.py)
按内容遍历资源、离线处理并核对结构/长度。Rudroid 的
[资源工具](../tools/README.md)采用内容识别、完整目录准备及序列化后独立校验的思路，
未复制上游代码。Maxwell→SPIR-V→Metal 面向 Switch/Apple，不能直接用于 Android
GLES；未知 shader 不改写，GPU 编译结果也不能跨驱动分发。

## NextOS

[运行时快照](https://github.com/initdream/nextos_ports_android/tree/16690ca320116d24fef3b8a4a82878b0cef1fa61)
展示 ELF/EGL/OpenSL/JNI 边界；[历史 Cuphead](https://github.com/DarkwingDuck13/nextos_ports_android/tree/2faa1f03d4eafd3f0dcfcb5f691b915f4f87d8b9)
提供音频阻塞案例。[NXExtract 说明](https://github.com/NextOs-Ports/nextos-universal-ports/blob/a72163f72a5019be7f340045e213d4d168d56938/tools/nxextract/README.md)
支持按内部内容选择 payload、准备后校验再发布的流程，但未取得提取器实现。
汇总仓库没有全部源码，不足以推断整个项目的公开状态或统一许可。

Rudroid 复用自己的 ELF/JNI/SDL 边界；不复制 RWX 写页、无限 detached 线程或未初始化
输出却返回成功的桩。OpenSL 不可用时明确失败，由 FMOD 选择 AudioTrack。

## d2r-mobile

固定 [内存改动](https://github.com/jhaizhou-ops/d2r-mobile/commit/e2a0c9a9b5d65f6cd505e967e4760e2745e90ca8)
及 [9393b00 快照](https://github.com/jhaizhou-ops/d2r-mobile/tree/9393b00ea9aca52d9254214da93738f26582562f)
用于比较按实际使用维持驻留、有界异步所有权、代次失效和状态去重。
[提交队列](https://github.com/jhaizhou-ops/d2r-mobile/blob/9393b00ea9aca52d9254214da93738f26582562f/dm/gfx/nvn/hle_nvn.c)
限制快照数量并在执行后退役；[渲染器](https://github.com/jhaizhou-ops/d2r-mobile/blob/9393b00ea9aca52d9254214da93738f26582562f/dm/gfx/nvn/nvn_mtl.m)
缓存反射摘要并区分 tile 优化的 CPU 成本与 GPU 收益。

Rudroid 已有有界 GL 元数据和关闭策略后的直通路径。Apple Metal 驻留及音频 QoS
不能直接移植到 Mali/Linux，也不为此增加 GLES 重放线程。未复制上游运行时代码；
引擎池缩小、长音频等本地取舍见 [性能决策](../crates/docs/PERFORMANCE-AUDIT.md)。

## Aurknix 音频与 DMC

固定 [5b530ab 快照](https://github.com/AveyondFly/distribution_rocknix/tree/5b530ab8bc3e7b88ef6d3eff9a55374c9b741dfb) 的
[RK3326 mGBA 补丁](https://github.com/AveyondFly/distribution_rocknix/blob/5b530ab8bc3e7b88ef6d3eff9a55374c9b741dfb/projects/ROCKNIX/packages/emulators/libretro/mgba-lr/patches/RK3326/0001-libretro-use-native-gba-audio-rate.patch)
依赖所用 libretro/mgba `c758314` 的同采样率直读分支，不适用于所有 mGBA，
也不能把 Unity/FMOD 输入直接标成 32768 Hz。
[RetroArch sinc](https://github.com/libretro/RetroArch/blob/df0f7e7635d38720515f41cfe7ba16563ef731c3/libretro-common/audio/resampler/drivers/sinc_resampler.c#L126)
按构建与 SIMD 条件选择实现；借鉴集中转换和分别核算成本，不能假设每个固件启用 NEON。

扩大缓冲抵抗短时抖动，同时增加排队延迟；容量不等于听感延迟，参见
[Libretro rate control](https://docs.libretro.com/development/cores/dynamic-rate-control/)。
[PX30 DMC/DFI 补丁](https://github.com/AveyondFly/distribution_rocknix/blob/5b530ab8bc3e7b88ef6d3eff9a55374c9b741dfb/projects/ROCKNIX/devices/RK3326/patches/linux/026-px30-dmc-devfreq.patch)
依赖 ATF ABI、共享内存、设备树与供电，用户态 loader 不能补齐缺失固件支持。
不复制板级频档或改系统 governor，见 [测量边界](../crates/docs/PERFORMANCE-AUDIT.md#音频与平台调参的证据边界)。

## Vulkan

采用 [Arm WSI ginkage 固定提交](https://github.com/ginkage/vulkan-wsi-layer/tree/8b18c35cb323e39b50776bd8ca8b1dbc7353f373)
的 Wayland 实现，以适配所测旧 Mali 驱动的扩展交集。Rudroid 可选后端将 Android
surface 转接 SDL Wayland，GBM allocator 提供准确 DMA-BUF/AFBC 布局；
只暴露真实支持的导入能力，不假报导出。其他硬件发行案例不等于本机验收。

[Unity Vulkan](https://docs.unity3d.com/6000.0/Documentation/Manual/vulkan.html)
要求具体 player 和 shader 支持；BuildSettings 的 GraphicsDeviceType Vulkan=21
与 ShaderCompilerPlatform Vulkan=18 是不同枚举。抽查资源和离屏清屏/回读、窗口
呈现探针各自只证明其阶段，不代表完整 shader、游戏或性能通过。
[Khronos WSI](https://docs.vulkan.org/guide/latest/wsi.html) 的 Android 与 Wayland surface
接口也不能直接混用。

上游关于透明度和同步的补丁需满足当前 compositor/驱动前提，不能仅换 fourcc 或
升级全部依赖就宣称修复。当前机型门禁、全尺寸合成及未通过的视觉情况见
[运行策略](../crates/docs/RUNTIME-POLICY.md#可选-vulkan-后端)、
[显示边界](../crates/docs/PERFORMANCE-AUDIT.md#显示与-vulkan-边界)和
[支持范围](../crates/docs/ACCEPTANCE.md)。

## 平台与 Mono

SDL 是统一接口，Wayland/X11/KMSDRM 的可用性取决于固件编译和权限，见
[SDL Linux](https://wiki.libsdl.org/SDL2/README-linux)。枚举成功不等于窗口、context 或
present 成功，已有合成器/SDL 旋转不能重复处理。

存在 Assembly-CSharp.dll 不等于系统 Mono 能运行 Unity；仍需匹配 player、runtime、
ABI、JNI/PInvoke 及生命周期。ARM32 与 Mono 执行后端尚未验证，按
[移植指南](../crates/docs/HANDHELD-PORTING-WORKFLOW.md)分阶段调查。
