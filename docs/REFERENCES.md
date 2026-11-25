# 参考项目与采用边界

本页保留固定来源、可借鉴机制及本地落点。外部项目的实现或性能数字不作为
Rudroid 的硬件、玩法或性能验收，第三方许可须按所用快照分别核对。

## PortMaster-Unity

[固定上游版本](https://github.com/wild1walker/portmaster-unity/tree/7f617e282953f8981ca4a38537277a8a9983ff90)
提供桌面 Unity 检查、数据转换、ARM Player 准备与端口模板。
Rudroid 的[主机工具](../tools/README.md#桌面-unity-移植准备)复用上游转换与制包 CLI。
[原生 Mono 运行时](../crates/docs/UNITY-MONO.md)由 Rudroid 的 `unityloader` 加载 Player、上游脚本桥与 Mono JIT。
标题资源、存档、商用 Player 和设备依赖由制作者分别准备。发行时须保留实际使用的组件许可。

### 桌面游戏的宿主差异

| 环节 | 已读到的上游机制 | Rudroid 的落点与验收 |
|---|---|---|
| Mono 启动与原生插件 | [mono64](https://github.com/wild1walker/portmaster-unity/blob/7f617e282953f8981ca4a38537277a8a9983ff90/runtime/host/mono64.c) 初始化桥，传入客体动态库解析函数，映射 Player 请求的库名。[插件表](https://github.com/wild1walker/portmaster-unity/blob/7f617e282953f8981ca4a38537277a8a9983ff90/runtime/host/plugins.c) 区分 Android ARM 库、Linux 库、指定兼容实现和缺失。 | Rudroid 的可选 `unity_mono` 读取制备清单与插件映射，通过现有 ELF loader 为 Player 注册桥。配置与依赖见[原生 Mono](../crates/docs/UNITY-MONO.md)。 |
| 游戏内的帧率与画质 | [framerate.c](https://github.com/wild1walker/portmaster-unity/blob/7f617e282953f8981ca4a38537277a8a9983ff90/runtime/host/framerate.c) 处理桌面游戏在 Android Player 上的目标帧率和垂直同步语义；[quality.c](https://github.com/wild1walker/portmaster-unity/blob/7f617e282953f8981ca4a38537277a8a9983ff90/runtime/host/quality.c) 对游戏运行时重新设置的画质应用可选上限。 | 明确区分应用设定的目标 FPS 与实际合成提交。移植某项策略前，在同存档、内部分辨率、骑行路线与前台合成条件下，分别开关该项验证画面、平均 FPS 和 P95 帧间隔。 |
| 过场视频 | [媒体实现](https://github.com/wild1walker/portmaster-unity/blob/7f617e282953f8981ca4a38537277a8a9983ff90/runtime/media/um_codec.c) 依设备与所带库选择硬解或软件解码。 | Rudroid 的 [media fallback](../crates/capability/android/media_fallback/AGENTS.md) 只提供明确的未支持返回。遇到过场阻塞的标题时，按编码格式验画面、声音同步、跳过和停止释放。 |

KTC 在同一设备、同一存档与显示设置下的结果见[原生 Mono 验收](../crates/docs/ACCEPTANCE.md#原生-arm64-unity-mono-验收)。

### 桌面 Mono 程序集如何在 ARM64 Player 上执行

| 候选 | 可复用的部分 | Rudroid 的采用条件 |
|---|---|---|
| [PortMaster-Unity Mono 桥](https://github.com/wild1walker/portmaster-unity/blob/7f617e282953f8981ca4a38537277a8a9983ff90/runtime/mono64/il2cpp_mono.c) | ARM64 Android Player 调用 `il2cpp_*`，桥转给相应 Unity 版本线的 Linux ARM64 Mono，游戏 DLL 由 JIT 执行。 | KTC 的 Rudroid 原型借用此桥进入第一岛。先对照现有实现；遇到线程身份、GC、异常或 P/Invoke 问题，检查实际触发的调用。例如该版本的 VM 线程查询返回固定值，外部 GC 跟踪接口为空。 |
| [桌面程序集离线 AOT](https://github.com/wild1walker/portmaster-unity/blob/7f617e282953f8981ca4a38537277a8a9983ff90/docs/il2cpp.md) | 从仍含方法体的托管程序集生成这款游戏的目标 `libil2cpp.so` 和 metadata。 | 当 JIT 或桥的开销、行为确实构成瓶颈时，用受支持的 Unity 工具链做同场景对照。需验证反射裁剪和原生插件；KTC 尚无 AOT 验收。 |
| [HybridCLR 构建流程](https://www.hybridclr.cn/en/docs/basic/buildpipeline) | Unity 项目构建时生成 AOT 桥并支持热更新程序集。 | 有工程和目标构建条件的游戏可单独评估。现有 KTC 输入只有发行数据。 |
| [BepInEx Il2CppInterop](https://github.com/BepInEx/Il2CppInterop/blob/81a6f78c8b653e0da4a3420ac4cd00819e8b6292/README.md) | 让 CoreCLR 托管代码与已经运行的 IL2CPP 对象和类型交互。 | 可研究互操作语义；KTC 需要由 Android Player 执行原桌面 Mono 程序集，调用方向不同。 |

KTC 的本地二进制对照（Steam Linux，Unity 6000.0.66f2）：在匹配的 Android ARM64 `libunity.so` 中检出 234 个以 NUL 分隔的 `il2cpp_*` 名称；与对照桥的 ELF 动态导出取交集，234 个均存在。原生宿主实验日志记录 Mono 载入 165 个程序集并进入第一岛。静态名称匹配用于发现明显缺失；各接口的签名、GC 和线程行为仍须用游戏或 fixture 验证。

### 资源、图形与原生插件

- 序列化资源与贴图沿用 Rudroid [现有工具及校验](../tools/README.md)。[UnityPy](https://github.com/K0lb3/UnityPy/tree/af1503d42776da0fcbd166e73497ed6064c3a060) 提供读写能力；压缩档按掌机内存和游戏画面验收。
- 着色器根据已有程序格式选工具。[Unity HLSLcc](https://github.com/Unity-Technologies/HLSLcc/tree/3ea1fcd6bd0ac445bc078de0bf32f0950188577b) 适用于支持的 Direct3D 字节码，[SPIRV-Cross](https://github.com/KhronosGroup/SPIRV-Cross/tree/57e4efcc84112abcd426dc5ec248b5339bee04f2) 接收 SPIR-V。转换后仍要检查目标 GLES 能力和实际画面；KTC 转换中有一个 VR 程序因 `gl_Layer` 被跳过。
- 只有桌面 x86-64 原生代码的插件或游戏，先找匹配的 ARM64 构建。Linux x86-64 二进制可用 [Box64](https://github.com/ptitSeb/box64/tree/31be4d4914939f604b6ddbd7e170b4742cc4218a) 做整进程对照。跨 ABI 的进程内插件调用需另外适配，Windows 输入还涉及 Windows API。

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
ABI、JNI/PInvoke 及生命周期。桌面 Mono 的 ARM64 路径已有 [KTC 真机验收](../crates/docs/ACCEPTANCE.md#原生-arm64-unity-mono-验收)。ARM32 和其他 Unity 版本仍按[移植指南](../crates/docs/HANDHELD-PORTING-WORKFLOW.md)分别验证。
