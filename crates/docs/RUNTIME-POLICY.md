# Rust 运行策略

本文只定义配置和所有权；效果取舍见[性能决策](PERFORMANCE-AUDIT.md)，
设备与标题范围见[验收](ACCEPTANCE.md)。

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

## 可选 Vulkan 后端

`gpu.graphicsApi` 默认为 `gles`，显式 `vulkan` 要求 Vulkan kit；拼写/类型错误、
缺少支持或后端不符时失败，不静默改变 API。当前候选限 Wayland、SDL 2.26+、
internal 呈现和单个 guest instance/device。100%／75%／50% 及精确渲染尺寸
共用原有尺寸策略，并以实际全屏输出尺寸计算比例。低分辨率使用 SDL 的局部
Wayland 模式与合成器线性缩放，不改系统显示模式；原生缓冲尺寸未落实则拒绝启动。
`gpu.vulkanRenderFilter` 只接受 `linear`（默认）；GLES 的 `gpu.renderScaleSharp`
保留原值，切回 GLES 继续使用。GL shader/texture
改写不作用于 Vulkan，比较时需保证相同画质与工作量。

系统 Mali ICD 保持原样；私有启动环境使用 `DISABLE_WSI_LAYER=1` 禁用固件旧
implicit layer，`VK_LAYER_PATH` 指向候选 WSI 目录，
`RUDROID_VULKAN_WSI_LIBRARY` 指向其中已审计的绝对 `.so` 路径。
桥接显式启用 `VK_LAYER_RUDROID_wayland`，Android surface 转为 SDL 拥有的
Wayland surface；GBM 精确布局的 DMA-BUF 由 Vulkan 导入，没有逐帧 CPU 拷贝。
WSI、Vulkan loader 和可选 SDL 查询函数均固定到进程生命周期。

没有系统 Wayland 时，启动器可复用已安装的 PortMaster Weston，只读挂载到独立
临时目录；客户端 SDL/Vulkan 库随可选包局部加载，GLIBC 最低 2.29，音频路径保留。
共享辅助程序拥有子进程、会话锁和挂载；游戏结束后停止合成器、卸载并释放显示租约。
游戏期阻塞等待子进程退出，不增加诊断轮询或后台采样；不修改系统 ICD、驱动或启动文件。
缺少依赖或原生预检失败时，菜单禁用 Vulkan。各游戏只调用同一入口，不复制检测流程。

preflight 只证明真实硬件和 surface 可用，报告零游戏帧；只有成功的 guest
`vkQueuePresentKHR` 才发送呈现回调。Unity 的固定 EGL 导入复用已有 facade，
没有 GLES session 时明确返回未初始化，不把 Android window token 交给原生 EGL。
实际游戏、输入/音频/退出、完整 shader 覆盖和帧率收益仍按标题验证。

配套启动器的丝之歌、吸血鬼 1.14在“高级 → 图形后端”选择，默认 GLES；
与运行库 `--graphics-features`、标题声明及有界 `--probe-vulkan` 原生探测
取交集后开放 Vulkan；探测不加载 guest、不呈现游戏帧，结束即释放窗口。
菜单直接选择 OpenGL ES / Vulkan，只有一个后端的标题隐藏选择。不可用值不能
选择，已存无效值在显示前改为可用默认值；启动前拒绝不可用的显式选择。
缩放无能力时用 100%，其他不适用项显示“禁用”并输出安全值。滤波自动处理。内存和游戏级画质保持独立。选择随游戏保存，
下次启动应用；后端与 Unity 启动参数一起原子写回，不改变内存档或其他参数。
游戏目录、配置根、资源和存档沿用既有布局；系统 ICD 和 GPU 驱动不修改。

## 可选 Unity 启动前渲染线程策略

`unity.render_thread_mode` 默认 engine，显式 single/multi 优先于 memory_mode 映射：

| 显式 memory_mode 时 | 策略/附加参数 | boot 三项值 |
|---|---|---|
| low_memory | single / -force-gfx-direct | 1 / 0 / 1 |
| balanced | engine | 保留 |
| performance | multi / -force-gfx-mt | 0 / 1 / 0 |

三项是 force-gfx-direct、gfx-threading-mode、gfx-disable-mt-rendering。
只接受精确原生兼容表；当前为 Unity 6000.0.61f1 ARM64，依据见
[原生构建 fixture](../common/host-config/tests/fixtures/unity-render-thread-build.md)。
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
HUD/配置 hook 的受限缺 getter 回退见[独立 ABI fixture](../common/tests/fixtures/legacy-gc-handle-abi.md)，
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

## Unity 工作线程配置

已有入口 `[package].mainIntentBundle = { unity = "-job-worker-count 1" }`，保留其他参数。
这是显式示例，默认由引擎选择；`unity.job_worker_count` 不是当前配置键。
减少 worker 仍需执行任务，可能增加主线程负担；不加自动线程重配或逐帧扫描。

## 帧率上限

两个 profile 默认 unity.target_fps=90、unity.render_sleep_us=0。
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
