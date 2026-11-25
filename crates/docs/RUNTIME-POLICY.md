# Rust 运行策略

本文只定义配置和所有权；效果取舍见[性能决策](PERFORMANCE-AUDIT.md)，
设备与标题范围见[验收](ACCEPTANCE.md)。

ARM64 Mono 的可信输入、`[unity.mono]` 和可选 Runtime 配置见 [Unity Mono](UNITY-MONO.md)。
新增 EngineProfile / 非 Unity 引擎见 [ENGINE-PROFILE-CONTRACT](ENGINE-PROFILE-CONTRACT.md)。

## Host API 版本与扩展槽

同 kit 成员共享一份 `HostApiV0`；**`HOST_API_VERSION = 7`**。只在表尾追加字段并升版本，
混 kit 拒绝。能力通过 `HostApiV0` 安全方法访问，不直接读编号槽位。

| 槽 / 方法 | 语义 |
|---|---|
| `register_service` / `register_named_service` | 命名服务首登胜出；`name` 与指针进程内固定；携带 ABI 版本 |
| `query_service` / `query_named_service` | 缺失或 ABI 不符返回 null |
| `register_service_typed` / `query_service_typed` | `ServiceContract` 类型安全封装；业务代码零 `unsafe` 消费命名服务 |
| `rudroid.host.config_toml.v1` / `config_toml` | Host 内置只读 `HostConfigTomlV1` 服务，供能力直接按同一 TOML 契约解析/拒未知字段 |
| `config_array_size` / `config_len` | 纯数组长度（缺失或非数组/表均返回 `0`） |
| `config_set_string` / `set_config_string` | 仅限 `package.mainIntentBundle.<key>` 的首写胜出运行时覆写（成功返回 `1`，其余键与重复写返回 `0`）；getter 先查覆写再查 HostConfig |
| `register_file_overlay` | 精确规范路径首登胜出；canonical → replacement（host 复制字符串） |

已有消费者示例：`unity_mono` 发布 `rudroid.unity.mono.v1`（validate / reserved
paths / start_bridge）；`android_elf_load` 发布 `rudroid.elf.guest_libdl.v1`；
Unity `BootView` 经 `register_file_overlay` 替换 sealed boot.config，并可经
`set_config_string("package.mainIntentBundle.unity", …)` 发布渲染线程参数。
服务在场不等于功能激活：Mono 计划与桥启动以 `[unity.mono]` 配置为准。
新跨能力状态优先命名服务，不要为单引擎私货扩展 Host API 字段。

## OBB 路径

`Context.getObbDirs()` 按 `[paths].android_obb_dirs` 的配置顺序返回 `File[]`，
`Context.getObbDir()` 返回首项。
相对路径以游戏工作目录为基准，已有路径会规范化，空字符串或非字符串成员占位为
`null`。未配置或数组为空时返回旧的单项 `android_external_files`，缺省为游戏工作目录。
查询只解析路径，不创建目录或授予存储权限。

## 内存模式的当前作用

loader 在初始化 capability 前解析 `[runtime]` 并统一发布，错误类型/范围拒绝。

```toml
[runtime]
memory_mode = "balanced" # low_memory / balanced / performance
# stream_buffer_bytes = 65536
# stream_buffer_budget_mib = 16
# preload_audio = true
```

| 模式 | 每条 Java 资源流 | 同 VM 载荷预算 | 支持的标题音频预加载 |
|---|---|---|---|
| low_memory | 0，直接读取 | 0 | 关 |
| balanced（默认） | 8 KiB | 1 MiB | 保留 |
| performance | 64 KiB | 16 MiB | 保留 |

显式缓冲/音频项覆盖模式默认；stream_buffer_bytes 为 0..1048576，
stream_buffer_budget_mib 为 0..1024。包内不可变 assets 的首次小块读取才申请缓冲，
大块读取不多复制；预算/分配失败回退直读，close/最后 owner 释放 File、buffer 和 lease。
canonical path 排除外部符号链接；普通文件、存档、pipe/device 直读以保留更新/阻塞语义。
Scanner 与 FileInputStream 共用对象内流，available 使用扣除预取后的逻辑位置。

preload_audio 当前由 silksong_unity6 的 AudioPreloader 补丁消费，标题显式
`game_patches.silksong_unity6.disable_audio_preloader` 优先；不代表所有游戏/FMOD
已统一预解码。内存档不自动联动帧率、画质、线程或额外回收。

## 模式差额与大内存边界

0/1/16 MiB 是按需缓冲载荷上限，不是 Unity 总 RAM、预分配量或 RSS 节省承诺。
堆、纹理、GPU 和 OS 缓存不在预算内；两档均饱和时 balanced/performance 相差 15 MiB。
电脑端资源转换的 1/2/4 GiB 压缩档是另一套离线策略。

## 内部缩放呈现的固定开销

`internal` 保留用户尺寸、滤镜和帧目标；非零 FBO/普通 getter 直接转发。
呈现按本次 GL 快照省略已满足的设置，保留 guest 状态和不相关 read FBO；
不建立猜测的跨帧状态缓存，也不因该精简更换后端。

`gpu.renderScaleHalfPrecision=false` 默认保留 highp；true 仅让 internal/sharp
的 RGB 运算使用 mediump，坐标、中心采样和透明度仍为 highp。启动创建 program 时
选择一次，不增加逐帧查询、采样数、锐化强度、缓冲或 pass；linear/100% 路径不使用它。
这是独立于内存档的设备/驱动可选策略，必须实际场景 A/B 验证后才写入部署配置。

## GLES 动态几何整缓冲失效实验

标题可显式设置 `gpu.glesWholeBufferInvalidate = true`。默认关闭。
只在主 GLES 3 窗口上下文、`GL_ARRAY_BUFFER`、`GL_ELEMENT_ARRAY_BUFFER`
或 `GL_COPY_WRITE_BUFFER` 目标、原始标志 `0x36`、偏移为零且覆盖整个
dynamic/stream 缓冲时，改用整缓冲失效标志。由驱动查询当前目标的实际大小，
索引缓冲的绑定取自当前 VAO。
驱动继续管理原对象、存储、刷新、VAO 和绘制。游戏查询映射标志时仍看到原值。
部分映射、无法验证的映射或超过跟踪上限的映射沿用驱动原路径。
该实验须使用 `--diag-probes` 构建并设置 `RUDROID_DIAGNOSTICS=1`。退出报告记录实际采用次数。

## 可选 Wayland 原生缓冲呈现

默认 internal；显式 wayland_native 由合成器放大同 guest 尺寸的 SDL 缓冲：

```toml
[gpu]
presentationBackend = "wayland_native"
renderWidth = 640
renderHeight = 480
renderScaleSharp = false
```

必须有精确尺寸和线性选择，不能静默去掉锐化或只用百分比。
正式 UI 没有此专用滤镜选项；启动器写回 sharp 会阻止启用。
仅 Wayland 尝试；失败先释放未激活状态再退回 internal，保持 guest 尺寸。
原生模式要求 RGBA8/depth24/stencil8，启动最多 250ms 等待窗口/drawable 达标。
Host API 输出取成功 drawable，物理桌面另记；输入不按桌面尺寸重复缩放。
SDL 请求结构生命周期与属性复位见 [SDL 约束](../capability/linux/sdl_runtime/AGENTS.md)。
不改系统模式/驱动或正式默认，收益按设备/标题验收。

## 日志与生产身份

正常运行仅保留显式 ERROR/FATAL、JNI 异常描述及致命错误；Android liblog 默认
只放行 ERROR/FATAL，guest 降低自身最小级别不能取消宿主的生产限制。
初始化、模块枚举、JNI 成功调用和渲染/纹理计数要求显式 `RUDROID_DIAGNOSTICS=1`；
CLI 查询、检查结果仍按命令输出。错误级别由调用方指定，不靠正文关键词猜测。
可选日志能力（如 `platform_log_stderr`）在 `EarlyInit` 阶段通过
`HostApiV0::register_log_sink` 向 loader 注册输出槽位，可按需替换；未部署日志能力时
loader 回退到内置 `stderr` 格式化输出。
启动器暂存第三方 UI 输出到临时目录，仅非零退出、显式错误或诊断会话才保存
最多 64 KiB 异常上下文，并在同样条件下记录内核退出日志；成功无异常时游戏日志为空。
显式诊断会话保留实时完整输出；正式捕获始终使用有界缓冲，避免临时日志占满内存或卡。
没有逐帧文件检查、后台采样或周期 sync。

构建默认 Release；Debug 只能显式选用且带非生产标记。正式打包/部署要求每个
loader/能力都有正向 Release 身份，拒绝 Debug、测试、训练及缺失身份的旧产物。
验收后重新纯生产构建和审计，不将诊断变体直接留在掌机。

## 可选 Vulkan 后端

`gpu.graphicsApi` 默认为 `gles`，显式 `vulkan` 要求 Vulkan kit；拼写/类型错误、
缺少支持或后端不符时失败，不静默改变 API。当前候选限 Wayland、SDL 2.26+、
internal 呈现和单个 guest instance/device。100%／75%／50% 及精确渲染尺寸
共用原有尺寸策略，并以实际全屏输出尺寸计算比例。低分辨率使用 SDL 的局部
Wayland 模式与合成器线性缩放，不改系统显示模式；原生缓冲尺寸未落实则拒绝启动。
`gpu.vulkanRenderFilter` 只接受 `linear`（默认）；GLES 的 `gpu.renderScaleSharp`
保留原值，切回 GLES 继续使用。GL shader/texture
改写不作用于 Vulkan，比较时需保证相同画质与工作量。

系统 ICD 或自定义 Vulkan 驱动按标准 loader 路径使用；当系统 ICD 已原生提供
`VK_KHR_wayland_surface` 时，可不设置 `RUDROID_VULKAN_WSI_LIBRARY` 直接走原生
Wayland ICD 路径。需要独立 WSI layer（如 Arm Mali 系统驱动）时，设置
`VK_LAYER_PATH`、`RUDROID_VULKAN_WSI_LIBRARY`（已审计绝对 `.so` 路径）和可选
`RUDROID_VULKAN_WSI_LAYER`（默认 `VK_LAYER_RUDROID_wayland`）；仅启用
`VK_LAYER_RUDROID_wayland` 时要求 `DISABLE_WSI_LAYER=1` 禁用固件旧 implicit layer。
桥接将 Android surface 转为 SDL 拥有的 Wayland surface；GBM 精确布局的 DMA-BUF
由 Vulkan 导入，没有逐帧 CPU 拷贝。WSI、Vulkan loader 和可选 SDL 查询函数均固定到进程生命周期。

可选的 `vulkan-malisx2/` 是独立 Linux ICD 包，不是 capability。启动器验证固定来源、
Release 收据、GNU ARM64 ELF 与库 SHA 后，单独执行原生预检；只有该候选成功才开放
“malisx2 Vulkan”。选择它时，仅游戏子进程设置 `VK_DRIVER_FILES` 与旧 loader 使用的
`VK_ICD_FILENAMES`，并开启下述 surface 载体。选择系统 Vulkan 或 GLES 时清除这些
候选设置。两种 Vulkan 使用相同的 `gpu.graphicsApi="vulkan"`，驱动选择不混入游戏
API 配置。当前候选限现有 Loong/G52 两标题及原生 Wayland；GLES 默认和原有系统路径
保持不变，候选通过预检仍不能证明完整绘制语义或玩法通过。
独立 ICD 不链接或加载任何 Rudroid capability，其他 Vulkan 应用也可按标准 loader
方式选择它。启动器可用 `LAUNCHER_MALISX2_DIR` 引用共享包；WSI 作为另一独立呈现层
按需配合。构建环境由本工程维护，不构成运行时依赖。

WSI 通过 loader 协商后的实例分派提供 Wayland surface 包装，并继续调用下层
`vkCreateWaylandSurfaceKHR`；动态导出表未列出该名字不等于没有内部实现。下层
入口缺失时，系统模式在创建本地 Wayland 对象前返回
`VK_ERROR_EXTENSION_NOT_PRESENT`，不增加虚假导出或伪造 surface。

仅明确设置 `RUDROID_WSI_HEADLESS_SURFACE=1` 的进程可选用 headless 载体适配：
layer 在实例创建时为 Wayland 请求给下游启用 `VK_EXT_headless_surface`，并把该
选择固定在实例内。存在原生 Wayland 创建接口时仍优先使用它；接口缺失时，必须
取得已启用的下游 `vkCreateHeadlessSurfaceEXT` 和销毁接口，创建真实的
`VkSurfaceKHR`，再将 Wayland 对象登记到原 surface map。查询与 swapchain 仍由
Wayland 实现，销毁仍配对下游 DestroySurface 与 map removal；注册失败会销毁
载体并保留 null 输出。fallback 不静默丢弃 Wayland `pNext` 或 flags：固定输入
版本不支持这些请求时拒绝创建。扩展未启用、接口缺失或关闭开关均不使用载体。
此开关不提供 GPU 驱动、GBM/DMA-BUF 或 compositor 能力，也不代表某个候选 ICD
已通过设备显示或游戏过程验收。

WSI 的 Release 仍保留 ERROR；普通日志仅在 `RUDROID_DIAGNOSTICS=1` 时开启，
此时 `VULKAN_WSI_DEBUG_LEVEL` 可调整详细程度，但不能关闭 ERROR。分配器只将
明确的 Linux 资源 errno 映射为 wsialloc v3 `NO_RESOURCE`；未知 GBM 失败沿用
`NOT_SUPPORTED` 回退并记录实际阶段/errno，该回退不证明格式不支持。调用方的
Vulkan 错误码也不能单独定位物理 OOM。此故障边界修正不构成 Rocknix 或其他
未验驱动组合的运行承诺。

没有系统 Wayland 时，启动器可复用已安装的 PortMaster Weston，只读挂载到独立
临时目录；客户端 SDL/Vulkan 库随可选包局部加载，GLIBC 最低 2.29，音频路径保留。
共享辅助程序拥有子进程、会话锁和挂载；游戏结束后停止合成器、卸载并释放显示租约。
游戏期阻塞等待子进程退出，不增加诊断轮询或后台采样；不修改系统 ICD、驱动或启动文件。
缺少依赖或原生预检失败时，菜单禁用 Vulkan。各游戏只调用同一入口，不复制检测流程。
TrimUI/A523 丝之歌显式设置 `RUDROID_WAYLAND_FORCE_RENDERER=1`，仅私有 Weston
接收对应的 `WESTON_FORCE_RENDERER=1`：客户仍按所选比例渲染，先合成为完整屏幕
尺寸再送屏，避免缩小缓冲直接扫描时的实屏条纹。预检与游戏采用同一策略，
不关闭 atomic/GBM modifiers、不替换驱动，不影响 MiniLoong 或其他标题。

preflight 只证明真实硬件和 surface 可用，报告零游戏帧；只有成功的 guest
`vkQueuePresentKHR` 才发送呈现回调。Unity 的固定 EGL 导入复用已有 facade，
没有 GLES session 时明确返回未初始化，不把 Android window token 交给原生 EGL。
实际游戏、输入/音频/退出、完整 shader 覆盖和帧率收益仍按标题验证。

配套五个 Rudroid 游戏启动器的新状态统一默认 GLES、75% 渲染，保留用户已有合法选择。
丝之歌、吸血鬼 1.14 仍在“高级 → 图形后端”手动选择并保存 Vulkan；
TrimUI/A523 丝之歌的缩小缓冲实屏条纹采用上述全尺寸合成策略，75% 场景与移动
已实屏核验；旧完整高特效读档黑屏等其他视觉失败不因这次修复视为通过。
启动器先按游戏、固件名称和完整 device-tree 条目匹配已验收组合；当前登记
丝之歌/吸血鬼 1.14 的 TrimUI + `allwinner,a523`、Loong +
`rockchip,rk3566-rk817-tablet`，不根据 IP 或笼统的 RK3566 名称推定支持。
未知组合在原生探测前禁用 Vulkan；新设备路径须经过实屏与玩法验收后再登记，
驱动自检不能替代视觉验收。再与运行库 `--graphics-features`、标题声明及有界 `--probe-vulkan` 原生探测
取交集后开放 Vulkan；探测不加载 guest、不呈现游戏帧，结束即释放窗口。
菜单直接选择 OpenGL ES / 系统 Vulkan / malisx2 Vulkan，只有一个后端的标题隐藏选择。不可用值不能
选择，已存无效值在显示前改为可用默认值；启动前拒绝不可用的显式选择。
缩放无能力时用 100%，其他不适用项显示“禁用”并输出安全值。滤波自动处理。内存和游戏级画质保持独立。选择随游戏保存，
下次启动应用；后端与 Unity 启动参数一起原子写回，不改变内存档或其他参数。
游戏目录、配置根、资源和存档沿用既有布局；系统 ICD 和 GPU 驱动不修改。

## 可选 Unity 启动前渲染线程策略

`unity.render_thread_mode` 默认 engine；只有显式设为 `memory_mode` 才按下表映射。
单独调整内存档不改变渲染线程模式，显式 single/multi 直接选择对应策略：

| `unity.render_thread_mode="memory_mode"` 时的内存档 | 策略/附加参数 | boot 三项值 |
|---|---|---|
| low_memory | single / -force-gfx-direct | 1 / 0 / 1 |
| balanced | engine | 保留 |
| performance | multi / -force-gfx-mt | 0 / 1 / 0 |

三项是 force-gfx-direct、gfx-threading-mode、gfx-disable-mt-rendering。
只接受精确原生兼容表；当前为 Unity 6000.0.61f1 ARM64，依据见
[原生构建 fixture](../capability/unity/common/tests/fixtures/unity-render-thread-build.md)。
启动检查有限 ELF header/note/build ID/文件长，不靠相邻版本或配置键猜兼容。
原始 boot.config 必须 ≤64KiB、UTF-8、三项唯一且为 0/1；参数 NUL、超长、
冲突或未知契约拒绝。engine 完全跳过预检/boot 投影，保留引擎行为。

加载玩家前生成只读、已取消目录链接的私有 File，各 reader 独立游标；原文件不改。
配置、策略、资源视图一次发布，失败不发布、竞争失败者 Drop，成功 owner 进程驻留。
不改 jobs/GC/音频/FPS/画质，不新增线程或每帧策略；multi 可多用 CPU/内存，
按标题显式选择，不能因 performance 名称自动启用。

## 可选扳机数字按键

`input.synthesize_trigger_keys=true` 默认保留 L2/R2 额外 KeyEvent。
false 只禁用这两个合成事件，保留模拟值、别名、归零和断开释放。

## 可选 Android 手柄身份

`input.android_xbox360_identity=false` 保留宿主身份；true 仅在创建 Android 手柄
记录时采用 Xbox 360/045e:028e，不改实际映射。名称/ID 由 VM 设备 owner 释放。
Build manufacturer 默认可配置的 Rudroid，避免 Linux 字符串误选桌面布局。
身份字段正确不代表标题所有按键正确，需实际动作验证。

## 可选原生库文件页回收入口

启动器显式使用 `crates/scripts/guest-exec-view.py`，在显示/输入初始化前重进一次
私有挂载 namespace，仅将给定 ARM64 原生库目录绑定只读 exec，然后 exec 原命令。
保留 PID/信号/退出码，无常驻解释器；父进程和系统挂载不变。
已 exec 或无权限时执行原命令/保留匿名复制回退；部分失败须先撤销，撤销失败拒绝启动。
源 SO 运行中仍不可外部改写；头检查不替代完整 loader 校验。
这不是 Unity GC/资源卸载，需写库目录的游戏不能直接套用。

## ELF 原生代码按需驻留

未修改且隔离的文件代码页保持 demand paging；流式跟踪重定位写入。
复制、text 写入、共享段页面和末尾清零页保留必要 I-cache 维护及 W^X。
noexec 复制后只一次丢弃冗余 clean 源页驻留，metadata 映射仍有效，可按需 refault。
不驱逐运行镜像或游戏资源，不新增后台回收；成功代码仍保持进程生命周期。

## Unity 内存压力回收

默认五标题不安装 unity_memory_policy。显式部署时还需 il2cpp_patch 和一个 profile，
plugins.enabled 若存在也须允许相应 id；插件默认启用，独立于 memory_mode。

```toml
[unity_memory]
enabled = true
reserve_mib = 64
recovery_mib = 128
cooldown_ms = 30000
warmup_ms = 5000
notify_low_memory = true
notification_grace_ms = 1000
unload_unused_assets = false
collect_managed = false
```

reserve 为 0..1048576 MiB，recovery 必须更大且 ≤1048576；cooldown 1000..3600000ms，
warmup 0..600000ms，grace 0..10000ms。初始化拒绝错误值，enabled=false 不额外采样/回收。
预热后在现有玩家维护事件最多每秒取 MemAvailable/cgroup 祖先 headroom 最小值，
旧内核退回 MemFree；采样失败跳过，reserve/recovery 滞回避免抖动。

压力且过冷却后先发低内存通知，跨帧宽限后重新采样；达到 recovery 取消额外卸载。
仍有压力且显式开启时才调用 Resources.UnloadUnusedAssets；grace=0、通知异常或
有效 headroom=0 可立即继续可用卸载路径。通知/引擎扫描仍可能卡顿。
unload_unused_assets=false 同时禁止本插件额外 GC，游戏自身回收继续。
collect_managed 只在额外卸载完成后生效，导出缺失独立降级，不伪造统计/成功。

只保留一个异步 GC root，玩家线程跨帧轮询，完成/停止/Drop 释放。
验证 static/参数/返回/byref；未知 ABI 不调用。当前额外卸载只允许完整构建
6000.0.61f1/ARM64 的指针宽 handle；旧版模拟适配不等于获准。
无参 void CallLowMemory 可经元数据单独验证；带 usage 的版本通知仍要求已验 ABI。
停止在 nativeDone 前撤销托管工作，在途调用返回后不得继续访问失效服务。
HUD/配置 hook 的受限缺 getter 回退见[独立 ABI fixture](../capability/unity/common/tests/fixtures/legacy-gc-handle-abi.md)，
不扩大压力回收版本表。详细生命周期见[memory_policy](../capability/unity/memory_policy/AGENTS.md)。
Unity 自己判断闲置引用/Addressables 配对；不以全包 UnloadAllAssetBundles 冒充标准回收。

### Linux swap 与标准内存统计

复用现有 swap/zram，不改 swappiness、不主动换出或清缓存；SwapFree 不加进可用 RAM。
v2 RAM/swap headroom 分开，v1 memsw 是合并额度；PSI 只诊断不设未经测量触发阈值。
RSS/PSS/SwapPss/GPU 计账不同，缺 PSS 不伪造，换出不等于释放同量物理内存。
维护在成功首帧后启用，不能消除初始化峰值或保证超容量游戏可玩。

## 可选渲染线程分配

`gpu.separateRenderThread` 默认 false；启用时还需 `gpu.renderThreadName` 精确匹配
Linux 的线程名（1..15 个 ASCII 可见字节）。当前启动器只为 TrimUI/A523 的丝之歌
写入 true 与 `UnityGfxDeviceW`，不新增菜单或联动内存、画质、分辨率、FPS。

SDL 在 guest 缩小亲和性之前保存宿主允许的在线核，要求恰有一个较高容量核和
两个等容量较低核。首次成功 GLES/Vulkan 呈现后，仅由实际呈现线程执行一次：
线程名必须匹配、不是进程主线程，且它和主线程仍只允许同一个较高容量核，才将
自身移到最初已允许的两个其他核。核编号不写死，后续帧不查询状态或扫描线程。
拓扑、权限、名称或亲和性不符就保留引擎策略；不改频率、在线核、优先级或其他线程。

## 可选 FMOD 混音线程分配

`sdk_fmod.separate_mixer_thread` 默认 false。FMOD addon 初始化时通过同一公共
ThreadPlacement 保存初始允许核及容量；它自己创建的混音线程在启动时调整一次。
仅当线程名称、主线程身份、两者缩小后的亲和性及已验证拓扑均符合时执行；
不扫描或保存外部 TID，不调整 native FMOD 音频流、Unity Job、频率或在线核。
不符合或设置失败时保留原策略，音频生命周期、PCM 队列及 stop/join 不变。

启动器仅为 TrimUI/A523 丝之歌的 Vulkan 路径开启；切换 GLES 显式关闭。
不增加菜单，不联动内存、分辨率或帧率；收益按设备/后端/场景验证。

## Unity 工作线程配置

已有入口 `[package].mainIntentBundle = { unity = "-job-worker-count 1" }`，保留其他参数。
这是显式示例，默认由引擎选择；`unity.job_worker_count` 不是当前配置键。
减少 worker 仍需执行任务，可能增加主线程负担；不加自动线程重配或逐帧扫描。

## 帧率上限

三个 Unity profile（共享 player_lifecycle）默认 unity.target_fps=90、unity.render_sleep_us=0。
用 monotonic 时钟量本轮输入/render/driver 等待，只补剩余周期，慢帧不补睡眠/追帧。
target_fps 接受 1..240，0 显式取消上限，其他值回退 90；render_sleep_us 接受
0..1000000，旧非零值与剩余周期取较大值而不相加。调用节奏不保证唯一显示 FPS。

## 验证与解释边界

配置解析、执行消费者和游戏效果分别验证。固定场景、尺寸/画质、帧目标和存档种子，
工具载荷减少或 Docker Mesa 通过不证明掌机 FPS/RSS 改善。
`unity.log_frame_progress=true` 开启 <9KiB 固定直方图，work 不含 pacing sleep，
分位为 250µs 桶上界、≥128ms 为 overflow；45–90 秒窗口不保证已进同一场景。

## 有界的内存只读诊断

`[unity_memory] diagnostic_only=true` 强制不通知、不卸载、不额外 GC。
diagnostic_interval_ms 默认 5000，范围 1000..60000；diagnostic_samples 默认 32，
范围 1..120，沿用 warmup。现有玩家事件执行，预算耗尽停止读取/日志，不另起线程。
managed heap/used 不与 PSS/GPU 简单相加；测帧率时关闭诊断。

## 可选 CPU 信息进程快照

`[runtime] cpuinfo_snapshot=false` 默认关闭。true 启动读取 ≤64KiB /proc/cpuinfo，
前后核对 online 拓扑，生成 kernel-sealed 匿名 File；仅精确该路径共享同一 owner，
新 reader 独立游标，close 归还 fd。不支持/读取失败保留原路径。
显式开启会冻结进程 CPU 文本，不适用动态拓扑 reader；其他 proc/sysfs 不缓存。
本机回切未见稳定净益，不能仅因重复读取就开启通用默认。

## 正常存档后的进程退出

`unity.fast_exit=false` 为默认值。丝之歌启动器自动启用：保留原生菜单及存档流程，
等待成功运行后的 `nativeRender=false`，同步刷新 SharedPreferences，确认成功后
由 CLI 的 `_exit` 结束进程，跳过 `nativeDone` 与析构。内存、线程和驱动句柄由内核
随进程回收；退出前不释放仍被工作线程引用的对象。该策略不创建监控线程或固定超时。
偏好写入失败保留脏数据并走完整清理；启动失败、测试帧数限制、快捷键退出同样保留
完整清理。它不替代游戏自身存档，也不作为其他标题的通用默认值。
