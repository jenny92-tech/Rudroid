# 当前能力与支持范围

本表列出能力实现、运行消费者、部署契约和未支持范围。
具体设备、游戏版本、测试与交付 kit 以[验收](ACCEPTANCE.md)为准。

## 产物与部署契约

loader 的 Cargo binary 及 deploy-select 输出仍为 `rudroid-unity-loader`；本机正式启动兼容文件名为 `unityloader`。
能力部署为 `lib<id>.so`。`reply_catalog` 构建产物是 `librudroid_android_reply_catalog.so`，
deploy-select 通过显式别名将它部署为 `libreply_catalog.so`。
实际输出、启动器兼容重命名与成员清单遵守[生产部署流程](../../BUILD-DOCKER.md#生产部署顺序)，不能只改某一端。
当前交付的 26 成员是五标题生产库存，完整 workspace 另含未选入的可选能力。

每个运行子集恰好一个 `android_base` 和一个 Unity profile。能力不直接相互链接；
普通库共用代码，Host API 和共享 VM 持有跨插件权威状态。C++ plugin id 保留，
新拆分能力也各有 id；旧 C++ host 内置项不必各新建一个 Rust 插件。

## Android 基础能力

| id / 实现 | 当前运行消费者 | 关键边界 |
|---|---|---|
| `android_elf_load` / [elf_load](../capability/android/elf_load/AGENTS.md) | AArch64 ELF、版本化符号、RELA/RELR/Android RELR、流式 APS2 REL/RELA、W^X、libdl、代码所有权 | 文件映射与 noexec 复制回退；源文件在游戏生命周期保持不变。DT_NEEDED 发现、TLS、新 guest 文件任意 dlopen 未实现。 |
| `android_bionic` / [bionic](../capability/android/bionic/AGENTS.md) | libc/libm/liblog、FILE、线程/同步、信号、网络、locale、FORTIFY、资产路径、析构 | barrier/spin 与 checked memory 已实现，不能沿用旧缺口表。guest layout/version 逐项门控；不提供整套 Bionic/NPTL/Android TCB。 |
| `android_jni_vm` / [jni_vm](../capability/android/jni_vm/AGENTS.md) | 单一对象/字段/引用权威、八类 primitive arrays、critical、typed scalar/nonvirtual、字符串、异常、native frame、流 | 只接受明确支持的 native 原型；不执行任意 Java bytecode，不能从 JNI slot 数推断完整兼容。 |
| `android_base` / [framework](../capability/android/framework/AGENTS.md) | Activity/Context/Looper、输入、原子偏好保存与全部现有 primitive 类型、路径/StatFs、Intent | Build manufacturer 默认可配置的 Rudroid；真实存储统计而非伪造容量。原始存档布局由标题配置保持。 |
| `android_assets` / [assets](../capability/android/assets/AGENTS.md) | NDK 文件/read/seek/mmap/fd/目录，复用 Java/Bionic 的资源权威 | 不整包复制；借出的指针必须在 close 前结束使用；不下载或解包 APK。 |
| `android_native_events` / [native_events](../capability/android/native_events/AGENTS.md) | native TLS、epoll/eventfd 的 NDK Looper，无新增 worker | 传入 fd/回调须保持有效；sensor 明确无设备，不等于掌机 IIO 接入。 |
| `android_zlib` / [zlib](../capability/android/zlib/AGENTS.md) | LP64 gate 后复用宿主压缩服务 | 需要目标 libz.so.1；流、回调和缓冲必须配对结束。 |
| `android_media_fallback` / [media_fallback](../capability/android/media_fallback/AGENTS.md) | 有类型的 NDK 不支持/无效对象失败 | 没有视频解码器，不伪造可用格式或成功。 |
| `reply_catalog` / [reply_catalog](../capability/android/reply_catalog/AGENTS.md) | 同 kit 的固定/工厂 JNI 回复 | 不是真实网络、鉴权、购买或广告服务。 |

## 宿主、Unity 与 addon

| id / 实现 | 当前行为及限制 |
|---|---|
| `platform_sdl_runtime` / [sdl_runtime](../capability/linux/sdl_runtime/AGENTS.md) | SDL 窗口/EGL/GLES、渲染与输出分离、内部缩放/可选 Wayland 原生缓冲、shader/texture 消费者、真实音频、校准轴/扳机/remap/热插拔/退出组合。硬件后端、物理控制器及标题操作分别验收。 |
| `platform_android_asset_packs` / [android_asset_packs](../capability/linux/android_asset_packs/AGENTS.md) | 已解出的包视图在 JNI prepare 前加入同一资源权威；不提供在线下载、许可或任意 APK 执行。 |
| `platform_log_stderr` / [log_stderr](../capability/linux/log_stderr/AGENTS.md) | 可选日志输出。 |
| `unity_2021_3`、`unity_6` / [旧版 profile](../capability/unity/profile_2021_3/AGENTS.md)、[Unity 6](../capability/unity/profile_6/AGENTS.md) | 版本 Spec 与补丁；两者复用 [player_lifecycle](../capability/unity/player_lifecycle/AGENTS.md) 的 prepare/bootstrap/render、Activity/反射/proxy/PAD 服务与停止流程。旧版软键盘、Unity 6 禁用 accessibility 委托均已有执行消费者；不是只注册类名。profile 名称不等于支持全部同系列版本。 |
| `il2cpp_patch` / [il2cpp_hook](../capability/unity/il2cpp_hook/AGENTS.md) | 保留并在 init 后安装 legacy/named 配置、广播 Il2cppReady、通用 HUD。签名/字段/构建均核对，未知原型拒绝，详见下节。 |
| `unity_memory_policy` / [memory_policy](../capability/unity/memory_policy/AGENTS.md) | 可选玩家线程低内存通知、宽限复查、显式异步清理及有界诊断。默认五标题不安装；额外 UnusedAssets/GC 默认关闭。 |
| `sdk_google_gms`、`sdk_google_play_core` / [gms](../capability/android/addon/gms/AGENTS.md)、[play_core](../capability/android/addon/play_core/AGENTS.md) | 共享 VM 本地完成 Task、主 Looper listener/proxy 与本地 ReviewManager 路径；不声明在线登录、评论提交或 entitlement。 |
| `sdk_levelplay` / [levelplay](../capability/android/addon/levelplay/AGENTS.md) | 主 Looper 异步失败桥，广告/AdQuality 不报告成功；不帮助启动广告或联网查询。 |
| `sdk_fmod` / [fmod](../capability/android/addon/fmod/AGENTS.md) | AudioTrack/共享 AudioOutput、实际 PCM、clocked/reuse/silence 策略与 stop/join。OpenSL 没有实现，明确加载失败以便引擎选择后端。 |
| `sdk_unity_burst`、`sdk_unity_helpers`、`sdk_xlua` / [addon 导航](../AGENTS.md#contents) | 共享 after_unity_module 的已知原生模块加载，保持构造/JNI_OnLoad 次序；不提供任意依赖树加载器。 |

## 通用补丁与游戏插件

配置请求会核对 assembly、唯一 overload、完整 static/参数/返回及 byref，按实际支持
选择 scale/freeze/force_return/trace/bypass；最多六个显式参数，ARM64 混合整数/FP
有专用调用桥。aggregate、裸 pointer、shared generic、未知栈形状或 build 明确拒绝。
原始 scale 在 original 前、freeze 在 original 后；force_return 保留 original 副作用，
bypass 才跳过 original。不能把 parsed/logged 当作执行完成。

HUD 使用 metadata 字段与受验证 GC root：隐藏 Component、GameObject deactivation、
scale、pivot 和位置/边距调整。裁剪 version getter 的精确原生证据回退只用于已核对的
HUD/配置适配，不扩大旧版内存回收。证据见 [GC-handle ABI](../common/tests/fixtures/legacy-gc-handle-abi.md)。

| id / 游戏 | 当前消费者与边界 |
|---|---|
| `hollow_knight_viewport` / [空洞骑士](../capability/unity/gamepatch/hollow_knight/AGENTS.md) | 视口/相机、post-callback UI 修正；固定版本 native 呈现兼容留在 profile，不全局开启。 |
| `silksong_unity6` / [丝之歌](../capability/unity/gamepatch/silksong/AGENTS.md) | 启动对象停用、postprocess、Sprite 裁剪、音频预加载、签名门控的菜单退出及语言保存；语言经正常退出后的新进程验证。TouchManager 由通用 il2cpp_patch 适配器处理。旧 C `silksong_viewport` 合并于本能力，不是独立 Rust 动态库。 |
| `terraria_android` / [泰拉瑞亚](../capability/unity/gamepatch/terraria/AGENTS.md) | 游戏 Activity 与共享 profile 配合；专用 external/internal 存储布局及 Android 输入配置保持原存档。 |
| `vampire_survivors` / [吸血鬼幸存者](../capability/unity/gamepatch/vampire_survivors/AGENTS.md) | 游戏特定补丁/资源策略；通用服务与显示/音频不按游戏名分支。 |

原生伴随库限定已批准的两个 Steamworks/SDK-ticket basename 及别名，由固定游戏目录
NOW|LOCAL 加载，防止 symlink 逃逸/重入；返回函数地址在 guest close 后仍保持代码有效。
设备 libm-copy fixture 验证加载/别名/handle/代码生命周期，不证明专有 Steamworks 行为。

## 剩余差异与未支持范围

| 分类 | 当前边界 |
|---|---|
| 架构与引擎 | ARM32、Mono、其他引擎执行后端未实现；ICEY 不能从存在 DLL 或共享 JNI 推导可运行。 |
| Vulkan | 可选 `--vulkan` SDL 后端已有 Android→Wayland、原生函数与 GBM DMA-BUF 桥接；MiniLoong 上丝之歌和吸血鬼幸存者已有有界玩法及退出验收。支持原生尺寸和已验证的 Wayland 线性缩放，保留 GLES 默认；其他设备、游戏及完整 shader 覆盖另验。见 [运行策略](RUNTIME-POLICY.md#可选-vulkan-后端)及 [Vulkan 核验](../../docs/REFERENCES.md#vulkan)。 |
| 动态链接 / native ABI | 自动 DT_NEEDED 发现、完整 ELF TLS、任意新文件 dlopen、RTLD_NEXT/guest-local namespace、Android TCB、process-shared 同步与线程取消未建立；逐项失败，不能裸 dlsym 到宿主。 |
| Java / Android | 没有完整 JVM/Java TLS/传感器/视频后端或真正 Google 在线服务。system CA 导出不等于 Java TLS runtime；广告失败/跳过策略不算在线适配缺口。 |
| 丝之歌可选项 | optional Destroy、prune 后额外 UnloadUnusedAssets、资源 census/method/audio traces 尚无运行消费者；已实现停用不等于 Destroy。标题 AssetBundle 卸载默认关闭，当前游戏效果未验收。签名检查绕过不提供。 |
| 参考诊断/时序 | 未提供 `platform_thread_trace`、HK 部分日志 wrapper；帧节奏由 target_fps/render sleep/vsync 控制，不启用参考实现的固定地址 frame-sync worker。诊断与时序差异不能直接算功能或性能回退。 |
| 设备验收 | 五标题短程范围之外的全玩法、所有实体控制器、物理热插拔、长期稳定性、1 GiB/其他固件与公平性能对照需另验收；不是这些路径均缺实现。 |

更新本表时同时查配置读取、注册/事件和实际调用方，不使用名称计数或历史缺口清单代替源码 review。
