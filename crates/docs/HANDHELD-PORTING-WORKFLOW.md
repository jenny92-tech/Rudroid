# 掌机游戏移植执行指南

按“已有能力 → 设备/游戏预检 → 最接近案例 → 最小运行 → 功能 → 优化/交付”推进。
当前实现是 ARM64 LE Android Unity IL2CPP → ARM64 Linux GNU/glibc；调查范围
可以更广，但 ARM32、Mono、其他引擎/宿主须单独实现和验收。

## 先查已有能力

| 需求 | 复用入口 | 核对内容 |
|---|---|---|
| loader/部署 | [unity_loader](../project/unity_loader/AGENTS.md)、[能力表](MIGRATION-MAP.md) | ABI、完整 kit、依赖及装载顺序 |
| ELF/Bionic/libdl | [elf_load](../capability/android/elf_load/AGENTS.md)、[bionic](../capability/android/bionic/AGENTS.md) | 重定位、导入、版本/布局，区分 Android guest 与 Linux host 库 |
| JNI/Java/Android | [jni_vm](../capability/android/jni_vm/AGENTS.md)、[framework](../capability/android/framework/AGENTS.md) | 完整描述符、对象/线程、路径、存档与生命周期语义 |
| 窗口/EGL/GLES/音频/输入 | [sdl_runtime](../capability/linux/sdl_runtime/AGENTS.md) | 实际固件后端、context/present、PCM、手柄轴/HAT/热插拔；多点触控未实现 |
| Unity | [player_lifecycle](../capability/unity/player_lifecycle/AGENTS.md)、[profiles](../AGENTS.md) | 完整 build、引擎入口、IL2CPP/GC ABI，不能以邻近版本代替证据 |
| SDK/native plugin/游戏补丁 | [能力表](MIGRATION-MAP.md) | 先复用 FMOD/Burst/GMS 等 addon；私有类型/偏移归 gamepatch |
| 离线资源 | [资源工具](../../tools/README.md) | GPU 格式支持与内存档分开；一次生成完整目录，默认 1 GiB |

为当前任务记录需求、已有实现、具体缺口、验证结果，避免复制另一套 loader。

## 设备预检

适配单位是设备＋固件＋启动方式＋游戏版本。平台记录未变时复用，换 SDL、驱动、
固件或启动方式只重验受影响项。通过 SSH/ADB 自行获取：

- CPU 与用户态 ABI、发行版、内核/页大小、GLIBC、实际 RAM/Swap/cgroup 和可用存储。
- 从固件前端进入时的环境、工作目录、挂载 exec/noexec、写权限和资源/存档路径。
- SDL 的编译/实际 video/audio driver；Wayland/X11 socket 或 KMSDRM/GBM/DRM master。
- 匹配目标 ABI 的 SDL/EGL/GLES/libmali 及其 DT_NEEDED；优先系统已有可工作 GPU 栈。
- 实际窗口/drawable、渲染尺寸、GLES 版本/扩展/纹理格式、输入设备和声音路由。

文件或 driver 名存在只是线索，必须创建 context、绘制并 present。
合成器负责的旋转不重复做；物理输出与 guest 渲染尺寸分别记录。
当前宿主由 SDL 实现，抽象边界不代表已有其他可运行后端。

## 游戏预检

先只读盘点 APK/split/OBB/PAD 或已有目录，不为基础分析安装大量新工具。
用现有 `file`、`readelf -h/-d/-Ws`、`nm -D`、`strings`、`objdump` 确认：

- 精确 Unity build、每个 SO 的 ELF ABI、依赖/导入，分析 main、unity 和脚本 runtime。
- IL2CPP 的 libil2cpp＋唯一匹配 metadata，或 Mono 的 libmono＋Managed 程序集；
  DLL 存在不代表有完整方法体，Windows x64 DLL 不能改名当 ARM64 SO。
- 所有 native plugin/PInvoke，以及实际 JNI 类、方法和完整描述符。
- Activity/Context、AssetManager、preferences、路径、AudioTrack/OpenSL、Surface、
  输入/传感器/日志需求；Java 依赖不能忽略，未支持服务须明确失败。
- base/PAD 的完整资源命名空间、stream sidecar、压缩格式、shader 与配置入口。

Mono 需独立确认 player/runtime 耦合、domain/程序集 ready、attach、GC root 与停止清理；
不能直接 `mono Assembly-CSharp.dll` 或伪造 IL2CPP ready。

## 参考与实施

优先用户指定案例；按相同引擎/后端、完整版本、CPU/宿主 ABI、平台 API、插件资源
选择最近实现。记录 revision、实际源码和许可，见[参考摘要](../../docs/REFERENCES.md)。
外部成功记录不能替代本设备验收，不复制固定分辨率、RWX、无限 detached 线程等策略。

在授权内自行检查、保护原始数据、修改、编译、运行、看退出码和日志，修首个失败点。
一次归因一个问题；独立修复可合成候选再编译，发布候选固定后统一回归受影响标题。
优先游戏目录内 LD_LIBRARY_PATH/shim/wrapper，不替换系统 libc/GPU/启动文件。
存档删除、不可恢复数据或系统核心修改才需要额外确认；备份策略遵从用户当前指令。
Rust 构建只走[固定 Docker 入口](../../BUILD-DOCKER.md)，不搭临时环境。

## 阶段通过条件

| 顺序 | 必须取得的证据 |
|---|---|
| 1. 文件与依赖 | 版本、ABI、脚本后端、导入/JNI/plugin/资源要求明确 |
| 2. 最小装载 | 实际 native 库映射、符号解析、构造函数及 JNI_OnLoad 完成 |
| 3. 引擎初始化 | 玩家与脚本 runtime 初始化成立；fixture 不充当真实游戏 |
| 4. 画面 | 窗口/context/present、标题和菜单正确；swap 返回不等于独立显示帧 |
| 5. 输入 | 菜单、移动、动作、轴/扳机/HAT、释放及正常退出组合有效 |
| 6. 音频 | 音乐/音效持续输出，区分 PCM、实体路由、回调及停止/join |
| 7. 存档 | 保存并退出，新进程重读；配置和原始数据范围明确 |
| 8. 稳定性 | 持续场景、重启、RSS/PSS/HWM/Swap、重载、OOM/崩溃记录 |
| 9. 性能/交付 | 同场景配置 A/B，完整生产 kit 和必要跨标题回归 |

不跨阶段宣布成功；记录实际观察和未通过项，不让用户代执行可自行完成的操作。

## 故障与性能

失败自动收集 linker/游戏日志、ELF/import 和内核记录。识别 not found、undefined
symbol、GLIBC/GLIBCXX、JNI、SIGSEGV/SIGABRT/Illegal instruction、EGL/GLES/SDL、
脚本异常与 OOM；黑屏先看日志和内存，不凭一条 GPU 日志猜根因。

从首次运行关注 managed/native heap、纹理、AssetBundle、音频和缓存，先跑正确再优化。
保留原尺寸/画质/帧目标做底层对照；分辨率是用户选项。比较帧时间长尾、每帧 CPU、
RSS/PSS/Swap 和重复加载；托管堆、PSS、GPU 页不能简单相加。
公共机制、库级插件和游戏专属配置各归其位，无净收益的策略保持关闭或显式选择。
诊断需要正向控制和零丢样，不能把菜单/加载/死亡混成同一场景。
现有取舍与必要负面结果见[性能说明](PERFORMANCE-AUDIT.md)。
