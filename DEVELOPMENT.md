# Rudroid 适配与开发规范

实现位于本仓库 `crates/`。相邻 Bogodroid C++ 仓库是只读行为参考。
当前支持与交付查[验收](crates/docs/ACCEPTANCE.md)，统一入口查[文档索引](docs/README.md)。

## 开始工作

| 工作 | 必读入口 |
|---|---|
| 修改源码 | [根 AGENTS](AGENTS.md) → [workspace AGENTS](crates/AGENTS.md) → 对应 crate 的 AGENTS |
| 新掌机/新游戏 | [移植指南](crates/docs/HANDHELD-PORTING-WORKFLOW.md)，先盘点已有能力，再检查真实设备、包与依赖 |
| 构建/回归/交付 | [BUILD-DOCKER](BUILD-DOCKER.md)，唯一现行命令及生产部署流程 |
| JNI/Host API/IL2CPP/unsafe | [common](crates/common/AGENTS.md)、相关 ABI fixture 与调用方 |
| 生命周期/卸载 | [loader](crates/project/unity_loader/AGENTS.md)、[player_lifecycle](crates/capability/unity/player_lifecycle/AGENTS.md) 与资源 owner |
| 策略与性能 | [运行策略](crates/docs/RUNTIME-POLICY.md)、[性能决策](crates/docs/PERFORMANCE-AUDIT.md) |

在已有授权内自行检查、备份、修改、编译、测试、读取日志，再修首个失败点。
先复用已验证的平台记录、配置和 capability，只补具体缺口；不能把指南的调查范围
当作 ARM32/Mono 等后端已经实现。不要让用户手动执行可自行取得的技术操作。

## 模块与策略归属

| 适用范围 | 归属与要求 |
|---|---|
| 通用 ELF/Bionic/JNI、句柄、文件与资源所有权 | 现有 android/linux 服务及 common；不识别游戏名、私有类型或 Unity 偏移 |
| Unity 生命周期与版本 ABI | player_lifecycle 共用流程，profile 的 Spec/原生门控表达版本差异 |
| 显示/音频/输入后端 | loader 的 host_platform 只负责启动选择，native 资源归实际适配器；音频/输入通过共享 VM 的 AudioOutput/InputSource 交互 |
| FMOD/Burst/helpers 等库级兼容 | 对应 Android addon，共用加载与服务，不复制进 loader 或各游戏 |
| 单游戏托管类型、HUD、资源或 native 补丁 | `unity/gamepatch/<title>`，package、版本/完整签名门控及显式配置 |
| 设备与标题的性能取舍 | 部署配置和有设备/固件/build/资源版本的实测记录，不隐式成为通用默认 |

当前实际宿主实现为 SDL；可替换边界不意味着其他后端已经存在。
只在启动期选择适配器，不为抽象增加逐帧动态派发、扫描或后台线程。

loader 保持薄入口。能力不直接链接其他 capability；共享代码放无 descriptor 的普通库。
每个 cdylib 都有自己的 common static，跨插件权威状态必须放 loader/共享 VM 服务。
每个运行子集恰好一个 android_base 和一个 Unity profile；Host API 扩展遵守尾部追加与版本规则。
原生 C/汇编只经 native-build 构建；新模块沿用显式文件布局，不新建 `mod.rs` 或 `build-*` 目录。
历史 Cargo/Docker/plugin/ABI 名称不能只改某一端，见[能力与产物](crates/docs/MIGRATION-MAP.md)。

## 内存安全与生命周期 review

业务代码使用 common 的 JniEnv、typed Host API、HostStr、HandleTable、IL2CPP/GC root
和 typed detour。重复原生操作集中到已有边界；safe 封装须对安全代码可构造的所有输入成立，
不能以“调用者通常不会传错”隐藏 unsafe 前提。保留必要 unsafe fn 契约与每块 SAFETY。

无 unsafe 的模块使用 forbid(unsafe_code)。`common/tests/unsafe_boundaries.rs` 固定入口与
各 crate 精确预算；预算变化必须随边界审查，计数减少或测试通过不等于生命周期已经证明正确。

| 边界 | 必须回答的问题 |
|---|---|
| 原生指针/句柄 | 谁拥有 backing memory？借用何时结束？失败、close、最后引用释放后能否再访问？ |
| JNI 引用 | local/global/weak 作用域是否正确？提升、发布与并发最后删除之间谁保住对象？ |
| 托管对象 | 跨帧持有是否有有效 GC root？正常、异常、停止、Drop 是否配对释放？ |
| IL2CPP | 完整 build/ABI、static、参数个数/类型、返回/byref 是否逐项核对？未知版本是否明确拒绝？ |
| 回调/线程 | 是否在正确线程？允许重入吗？锁是否跨 guest/外部调用？重入停止能否记录并在返回后阻止托管访问？ |
| DSO/GPU | 函数地址与在途调用结束前代码仍有效吗？context 是否 current，owner 线程与销毁次序正确吗？ |
| 初始化/退出 | 分配失败、部分初始化、重复清理和重入退出能否释放一次，并停止访问失效服务？ |

当前成功 capability、guest 代码及 SDL session 保持进程有效期，不自行增加成功路径的
动态卸载/销毁。nativeDone 前等所有在途玩家回调返回；不持有 registry/exec-table 锁调用外部代码。
共享 terminal 流程不额外注入 nativePause，音频 stop/join 后才释放。C 的 `_exit(0)` 不证明此流程。
GC-handle ABI 只用精确证据；HUD 的受限缺 getter 回退不能扩大旧版资源回收门控。

## 构建、测试与性能证据

构建/检查/测试仅经[固定 Docker 入口](BUILD-DOCKER.md#固定环境)，复用指定镜像、容器与缓存。
构建期间禁止修改 workspace 源码、manifest、Cargo.lock。测试后按[生产部署顺序](BUILD-DOCKER.md#生产部署顺序)
整套重建并审计同 kit、成员、test-marker 和 ABI；不得把测试留下的 cdylib 或训练版交付。

- 按影响范围组织验证：同模块可独立审查的修复合成候选，一次编译后做相关接口/生命周期检查，
  再选覆盖受影响后端/Unity 版本的实机路径。固定发布候选后统一做必要跨标题回归。
- 性能归因保持单变量或明确组合，先离线分析已有日志，不为微小收益反复走全部菜单。
  保持画质、渲染/输出尺寸、滤波、FPS 目标和存档种子一致；比较每帧 CPU、帧尾、PSS 和重载。
- 追踪先验证实际入口，按字段精确检查 overrun、commit overrun、dropped events，缺失也拒绝。
  不用 substring 匹配零值；关闭 setup 追踪、限制 buffer/时长，API/CPU 追踪与帧窗口分开。
- 缺环境明确失败或带原因 ignored，不提前 return 伪装成功；嵌套/子进程/定向复测不重复计数。
  `--check` 会 dlopen 并执行构造函数，只检查可信 kit；`RUDROID_SKIP_GUEST` 在窗口前返回。
- 纯文档修改只检查链接、锚点与格式，不启动游戏、部署或重新编译。

## 已知适配陷阱

| 误判 | 正确处理 |
|---|---|
| 芯片 64 位就能用 ARM64 kit | 检查用户态与实际依赖 ELF；架构、GLIBC、页大小和驱动分别确认。 |
| SDL 有 driver 或 Docker Mesa 通过就算能用 | 实机确认窗口/context/present、输出与 guest 尺寸、声音和输入。 |
| 配置能解析就代表功能可用 | 跟到消费者与实际执行；当前支持范围查[能力表](crates/docs/MIGRATION-MAP.md)。 |
| 统一前缀只是文字替换 | 核对所有导出及内部 native linker 名；zlib 的公开 resolver 与内部 C 查询必须分名，避免错误绑定成递归。ELF 夹具的名称长度、字符串表大小与偏移也要同步。 |
| mmap hint 就是可用跳板页 | 指定页只用 MAP_FIXED_NOREPLACE 并核对返回地址；失败不得覆盖已有映射。 |
| 已有数字按钮就是完整手柄 | 核对名称/ID/厂商、轴/range、扳机、HAT、remap、热插拔释放与各标题操作；Android Build 默认 Rudroid 避免误选 Linux 布局。 |
| nativeRender 返回就是游戏帧 | 看标题、菜单和关卡，确认移动、声音、保存/重读、退出与新进程启动。首帧前 false 可重试；运行后 false 表示引擎停止，不能再按 20,000 次启动重试处理。游戏菜单退出与 Select+Start 分开验证。 |
| 拦截 Application.Quit 只影响 Android 启动器 | 全局拦截也会吞掉正常菜单退出。丝之歌游戏逻辑在 Silksong.dll；按钮之后的 GameManager.QuitGame 协程仍需处理。仅在完整元数据校验的 UI/MoveNext 原调用链内放行，作用域跨异常恢复、不在帧间保留许可；不能全局关闭拦截而让启动期请求提前结束游戏。 |
| “跳过启动页”就不会显示 | 自动继续仍可能先显示数秒。丝之歌保留原 Start、按钮事件和初始化，只在首次绘制前隐藏已验证的子面板；不得提前禁用负责推进流程的启动器对象，或增加游戏帧轮询。 |
| 丝之歌正常退出长时间停在 nativeDone | 仅该标题在引擎正常请求停止、偏好同步写入成功后启用进程退出；保持所有活状态映射，直接走 CLI `_exit`。不得提前切断游戏存档、在托管回调/固定超时杀进程，或把它设为其他标题默认。失败、快捷键和测试帧数限制保留完整清理。 |
| guest 失败就直接普通进程退出 | 先显式停止并 join 服务、刷新首选项，再以非零状态结束进程；不得通过 host atexit 重进已由 nativeDone 清理的 Unity 状态。guest 尚未进入时保留普通错误退出。 |
| 资源减少或池容量变小就是优化 | 区分实际采用值、虚拟容量、驻留与缓存；少量内存不抵消额外扫描、堆外分配或卡顿。 |
| UnityPy 能重读就说明原生 Unity 能加载 | 流式纹理内联并重排 `.assets` 曾在 Unity 6 引发越界与 OOM。保留原序列化头、对象位置和流式存储；校验原流前缀与新增字节归属，仍需真机验收。 |
| SD noexec 只影响可执行文件 | guest SO 也会匿名复制；已验收启动器可显式用 exec 视图，源 SO 全程不可改写；不替换系统挂载。 |
| 批量卸载等于 Unity 自动回收 | UnloadAllAssetBundles 与 UnloadUnusedAssets 分开验收，额外 GC/UnusedAssets 默认 false，默认五标题不部署 memory_policy。 |
| 配置写入就代表生效 | 路径固定到真实游戏根；曾因 Scanner 只接 `\A`、不接 Unity 使用的 `\z` 而整段 boot.config 未读。追踪消费者后再比较线程和性能。 |
| 有语言整数就说明下次能恢复 | 丝之歌旧 `GameLang` 整数不是启动消费者读取的键。成功切换后保存实际枚举名到 `M2H_lastLanguage`，经全新进程验证；不在通用首选项层硬编码游戏键或语言。 |
| 包装器 safe 就能忽略原生参数生命周期 | SDL2 0.37 的模式包装曾借用已结束的临时结构；改用具名、初始化的 C 值，保持至 native 复制结束。像素属性只在 context current 后查询。 |
| 无事件就说明没有重载 | 先用实际入口正向控制，并精确核对丢样字段；未附着或丢样窗口不能作负面证明。 |
| 换成纯 Wayland 抓屏就完全安全 | LoongOS 1.4.0.36 的 Weston 在 Vulkan 启动期和 LÖVE 设置页均触发过 pending_capture_list 断言；等待渲染稳定不是充分修复。停用该外部捕获接口，设置页可用 LÖVE 自身捕获；诊断时只读当前线性 DRM 缓冲，校验格式/布局、成对同步并释放 fd/handle，不做 modeset，避开性能窗口。合成器丢失后的退出 0 不算玩法通过；保留 Weston 日志，与游戏崩溃及旧 RGA 内核异常分别归因。 |

## 文档与完成条件

测试放 tests/unit、tests 或 tests/fixtures，src 只放实现；dlopen 测试先经 test-support
按同 profile/features 重建。原始 APK、游戏、存档与缓存仅为本地输入，不纳入源码；
显式实游输入用同一 Docker 入口的只读挂载，见[输入规则](BUILD-DOCKER.md#本地真游戏输入)。

接口/职责变化同步源码 INPUT/OUTPUT/POS、crate AGENTS 与上级导航，遵守[文档协议](FRACTAL-DOCS.md)。
保留上游许可、作者和来源。当前规则只保留一处，历史测量归档并附日期/范围。

完成报告说明改了什么、依据、实际检查、设备/游戏范围及未验证项。
适配记录包含设备/固件、完整游戏/Unity 版本、配置、kit 与原始证据；优化附可复现对照。
新交付重新执行必要门禁，不拿上一轮通过代替本轮检查。
