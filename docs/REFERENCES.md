# 参考项目与采用边界

只保留来源、可复用机制和本地落点。外部项目的性能数字不作为 Rudroid 验收。

## nxnative

2026-10-17 核对 [9579030](https://github.com/seedds/nxnative/tree/9579030e5b1f0443c12b7376559560915b6d4c4b)。
其 [shader_split](https://github.com/seedds/nxnative/blob/9579030e5b1f0443c12b7376559560915b6d4c4b/dm/gfx/tools/shader_split.py)
按内容遍历资源，离线处理并检查结构/长度；一条入口完成准备。
Rudroid [资源工具](../tools/README.md)已采用：UnityFS 魔数识别、不依赖文件后缀、
离线格式转换、原始像素重试、完整 mip、序列化后独立校验及完整目录发布。
BC/DXT 颜色纹理可转 ASTC；CPU 可读、字体、数据/HDR 保持原样。
未复制上游代码。其 Maxwell→SPIR-V→Metal 翻译针对 Switch/Apple，不能直接用于
Android GLES。未知 shader 不改写；GPU 原生编译结果也不能跨驱动直接分发。

## NextOS

已审查的 [运行时快照](https://github.com/initdream/nextos_ports_android/tree/16690ca320116d24fef3b8a4a82878b0cef1fa61)
包含 ELF/EGL/OpenSL/JNI；[历史 Cuphead](https://github.com/DarkwingDuck13/nextos_ports_android/tree/2faa1f03d4eafd3f0dcfcb5f691b915f4f87d8b9)
提供音频阻塞案例。汇总仓库没有全部源码，不能据此认定整个项目未公开。
[NXExtract 说明](https://github.com/NextOs-Ports/nextos-universal-ports/blob/a72163f72a5019be7f340045e213d4d168d56938/tools/nxextract/README.md)
支持按内部内容选 payload、准备后校验再发布的思路；该次未取得提取器实现。
不复制 RWX 写页、detached 无限线程或不初始化输出却返回成功的桩。
本项目继续复用既有 ELF/JNI/SDL；OpenSL 不可用时明确失败，由 FMOD 选择 AudioTrack。
不同快照的许可分别核对，不能把 README 声明当作所有文件的统一许可。

## d2r-mobile

[2026-10-14 内存改动](https://github.com/jhaizhou-ops/d2r-mobile/commit/e2a0c9a9b5d65f6cd505e967e4760e2745e90ca8)
启发按实际使用维持驻留、区分虚拟容量与物理占用。
Apple Metal 驻留管理不直接移植到 Mali/GLES；引擎池实验未取得净收益，
保持[性能决策](../crates/docs/PERFORMANCE-AUDIT.md)中的保守默认。

2026-10-18 复核到 [9393b00](https://github.com/jhaizhou-ops/d2r-mobile/tree/9393b00ea9aca52d9254214da93738f26582562f)，
比上次 7ff4e36 多 16 个提交；最新提交日期为 10-17，旧的约 0.5 GB 驻留声明不是新增收益。
[提交队列](https://github.com/jhaizhou-ops/d2r-mobile/blob/9393b00ea9aca52d9254214da93738f26582562f/dm/gfx/nvn/hle_nvn.c)
用 32 项上限和执行后退役保护异步快照；
[渲染器](https://github.com/jhaizhou-ops/d2r-mobile/blob/9393b00ea9aca52d9254214da93738f26582562f/dm/gfx/nvn/nvn_mtl.m)
按对象缓存反射摘要、按编码器去重状态，并分别测量 tile 优化消耗的 CPU 与节省的 GPU 工作。
可复用有界所有权、代次失效和逐项核算成本的方法；Rudroid 已有相应的有界 GL 元数据及关闭策略的直通路径。
Unity 渲染线程沿用已验 build 的现有入口，不额外引入 GLES 重放线程；Apple 音频 QoS 也不直接搬到 Linux。
未复制上游运行时代码。原始对照及本轮真机音频实验保存在本机 `crates/target/d2r-memory-followup-20261018/`。
本轮交付后再次查询默认分支，仍为 `9393b00`，相对上述审查新增 0 个提交；没有新增优化可移植。

## Vulkan

2026-10-18 在当前 MiniLoong / RK3566 / Mali-G52 / LoongOS 上运行系统
`vulkaninfo --summary`，退出 0；GPU API 为 **1.2.218**，loader 为 1.3.207。
固件默认扩展没有 `VK_KHR_wayland_surface`，原有 WSI layer 仅支持 headless。
独立枚举不能证明游戏可运行。

随后独立实测 Vulkan 1.0 的 64×64 离屏图像：清屏、GPU 拷贝、fence 等待及 CPU 回读，
4,096 个像素均为预期 RGBA；禁用/启用现有 WSI layer 两次均退出 0、正常释放，
内核没有新增错误。只用 Mali ICD，图像/回读缓冲显式分配合计 33,024 字节。
这证明基础 GPU 命令可用，不证明着色器执行、窗口呈现、Unity 初始化或性能收益。

Wayland 合成器已提供 linux-dmabuf、wl_compositor 和 xdg-shell；驱动具备
image DRM modifier、dma-buf、external memory/fence fd 等关键扩展。
现已复用 [Arm WSI 的 ginkage 分支固定提交](https://github.com/ginkage/vulkan-wsi-layer/tree/8b18c35cb323e39b50776bd8ca8b1dbc7353f373)
的 Wayland 实现；最新分支要求的部分扩展不适用于当前旧驱动。
[ROCKNIX 20241029](https://github.com/ROCKNIX/distribution/releases/tag/20241029)
记录了 ginkage 参与的 RK3588 Vulkan 支持，属于其他硬件案例。

首测候选为丝之歌：实际 BuildSettings 含 Vulkan/GLES3 `[21, 11]`，抽查的 46 个
内置 Shader 均有非空且可解压的 Vulkan 平台块。吸血鬼同样声明 `[11, 21]`。
GraphicsDeviceType 的 Vulkan 是 21，ShaderCompilerPlatform 的 Vulkan 是 18，
两种枚举不能混用。尚未检查全部关卡 shader 或验证其实际执行。

[Unity 的 Vulkan 后端](https://docs.unity3d.com/6000.0/Documentation/Manual/vulkan.html)
仍要求具体游戏包含对应 player/shader 支持。
[Khronos WSI](https://docs.vulkan.org/guide/latest/wsi.html)区分 Android 与 Wayland 的 surface 创建接口；
Rudroid 的可选后端已将 Android surface 转接到 SDL Wayland 窗口，并接入原生
Vulkan 函数和呈现回调。当前驱动只支持所测 DMA-BUF 格式的导入，不能假报导出能力；
Rust GBM allocator 提供准确 AFBC 布局，首个真实 buffer 参与 WSI 图像布局选择。
独立 640×480 探针完成 120 次呈现，每帧验证 64 个 GPU 回读像素，正常退出。

前一 d83 套件已在丝之歌和吸血鬼实际完成 Vulkan 游戏路径与 GLES 回切对照。
不同渲染尺寸与套件的旧数字不能拼接成收益；当前吞吐、CPU、额外 GPU 分配及短窗
局限统一见[性能审计](../crates/docs/PERFORMANCE-AUDIT.md#可选-vulkan-对照2026-10-18)。
系统 GPU 库保持原样；可选后端的限制见[运行策略](../crates/docs/RUNTIME-POLICY.md#可选-vulkan-后端)。
本机枚举与 layer 定义证据：`crates/target/language-release-20261018/vulkan-*.json`。
离屏命令、shader/构建设置及后续结论：`crates/target/vulkan-feasibility-20261018/`；
早期窗口记录在 `crates/target/vulkan-backend-20261018/`；当前五标题与后端复测在
`crates/target/final-device-20261018/`。
试验使用私有配置/存档和只读资源视图，没有修改原游戏、正式配置或系统驱动。

丝之歌后续保存/快速退出及新进程 Vulkan 重读在当前 cb21 套件验证；证据在
`crates/target/sh-cleanup-exit-20261018/`，覆盖范围见[验收](../crates/docs/ACCEPTANCE.md)。

## 平台与 Mono

SDL 是统一接口，Wayland/X11/KMSDRM 是后端，取决于固件编译和权限。
查 [SDL Linux 说明](https://wiki.libsdl.org/SDL2/README-linux)及实际库；枚举成功不等于
窗口、context、present 成功。已有合成器/SDL 旋转不能重复做。
有 Assembly-CSharp.dll 不等于系统 Mono 能运行 Unity；必须匹配 player、runtime、
ABI、JNI/PInvoke 和生命周期。当前 Rust 未实现 Mono/ARM32 执行后端。
设备及游戏核验按[移植指南](../crates/docs/HANDHELD-PORTING-WORKFLOW.md)，支持范围查[验收](../crates/docs/ACCEPTANCE.md)。
