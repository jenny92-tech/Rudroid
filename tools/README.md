# Unity 主机工具

本目录是**离线主机工具**的唯一实现区。仓库内全部可执行入口（含构建脚本与
运行时调试开关）的登记与选用规则见 [TOOLING](../docs/TOOLING.md)；新增 CLI
前先查该表，勿平行再挂一套转换/审计命令。

- [一次生成 Unity 游戏资源](#一次生成-unity-游戏资源)
- [桌面 Unity 移植准备](#桌面-unity-移植准备)

## 一次生成 Unity 游戏资源

输入原始游戏资源目录，默认按 1 GiB 档位生成完整资源目录：

```sh
.venv-unity/bin/python tools/unity_assets.py /games/original/gamedata /games/my-port/gamedata
```

| 目标 | 用途 |
|---|---|
| `astc-1g` | 1 GiB 设备，优先减少纹理载荷 |
| `astc-2g` | 2 GiB 设备，兼顾资源占用和画质 |
| `astc-4g` | 4 GiB 设备，采用更保守的压缩策略 |

外部程序可用 `--target astc-2g` / `--target astc-4g` 或 `prepare(..., target=...)` 覆盖默认档位。
这些目标要求 GPU 支持 ASTC；Mali-450 不适用。内存大小不能代替 GPU 能力判断。
档位决定压缩策略，游戏的实际 RAM 和帧率仍取决于引擎、场景及原生库。

当前 4 GiB MiniLoong 已实际加载《吸血鬼幸存者》的完整 `astc-1g` / `astc-2g` /
`astc-4g` 输出并进入短程战斗，1G/2G 另测移动、进程音频和正常退出。
没有证明稳定提帧；1/2 GiB 硬件、其他游戏的完整转换和长时间游玩尚未验收。
丝之歌另有定点 mip 修复的关卡试验，不能扩大为全部修复贴图均已显示。
为保留原资源布局，输出文件可能变大，纹理载荷减少不等于运行内存同比减少。
实测和已知失败见[性能决策](../crates/docs/PERFORMANCE-AUDIT.md#离线纹理转换实测)。

一次命令内部完成检查、策略选择、编码、画质回退和输出校验。画质不通过时自动尝试
更细的分块，仍不通过就保留原图。字体、数据/HDR/CPU 可读纹理保持原样，保留尺寸
和全部原有 mip，不做运行时压缩或分辨率修改。
桌面 BC/DXT 颜色纹理也会自动尝试转换，避免把相同文件字节数误当作无需适配。
按文件头识别 UnityFS，无后缀或改名的资源包也会处理；其他文件保持原样。

输出保留全部相对路径、音频 sidecar、原生库和其他输入文件，无需手动拼资源。
原始目录不变，输出必须是独立的新目录。失败清理临时目录，不留下半成品。
不生成缓存、副本链、手工分层清单或多阶段日志；结果只附一份 `.rudroid-resources.json`。
转换前先用短命进程扫描全树字体引用，跨资源包的 Font/TMP→材质→纹理也保持原样；
父进程只保存引用键，不同时持有各资源包或像素。普通材质不会因此全被禁用优化。
完成引用预扫后，未知纹理布局的资源包逐字节保留并记入结果，未知纹理格式保留原载荷。
字体或材质引用元数据无法可靠读取、或者资源归属有歧义时，中止且保留原输入，不猜测后继续压缩。
流式纹理保持流式存放，保留原 `.assets` 的对象位置；内联或外部纹理布局保持原样。
最终目录通过系统的原子“不覆盖”操作发布；另一个任务即使只创建了空目录，也不会被替换。

首次准备 Python 3.10+ 环境：

```sh
python3 -m venv .venv-unity
.venv-unity/bin/python -m pip install -r tools/unity_astc/requirements.txt
```

只读审计和旧 mip 尾层定点修复保留为开发工具，不属于用户执行步骤。
Windows IL2CPP 游戏逻辑恢复也属于独立的游戏适配，资源转换不会补全缺失逻辑。

编码与质量算法源自 Bogodroid 的 `astc_retier.py` / `analyze_plan.py`，现已删去旧预算
模型、多套 CLI、流重排和透明化选项。这些维护者自有工具采用根 `LICENSE` 的
非商业源码共享协议；第三方依赖保留原授权，范围见根 `NOTICE`。

## 桌面 Unity 移植准备

`unity_port.py check` 在开发机上识别游戏的来源平台、Unity 版本和脚本后端。对于桌面 Mono 游戏，用 `prepare` 制备数据，再用掌机上的 `unityloader` 启动。
检查与制备复用固定版本的 PortMaster-Unity 工具，需要 Python 3.11+。
在所用的 Python 虚拟环境中安装上游源码 `convert/requirements.txt` 的依赖。
用 `--source PATH` 或环境变量 `PMUNITY_SOURCE` 选择源码，`--source` 优先。
透传命令保留调用者的参数与当前目录。Rudroid 自有的 `prepare` 步骤在私有工作目录中转换并打包。

使用 [固定提交](https://github.com/wild1walker/portmaster-unity/tree/7f617e282953f8981ca4a38537277a8a9983ff90)
的 Git 检出，或把 `PMUNITY_SOURCE` 指向 [该提交的 tar.gz](https://codeload.github.com/wild1walker/portmaster-unity/tar.gz/7f617e282953f8981ca4a38537277a8a9983ff90)。
入口检查 Git HEAD；有本地改动的检出与解压目录都可以运行，并提示未验证。
固定归档校验 SHA-256 `6cad05941b1d1c435021664936b7bff849de09685701247d1aec13e2bbec5c4e`
后临时解压，命令结束后清理。需要预编译上游工具或应用本地扩展时使用独立源码目录，
只运行信任的源码、插件与配置。

```sh
export PMUNITY_SOURCE=/path/to/portmaster-unity
python tools/unity_port.py check "$GAME" --json
```

普通文本输出中的 `Rudroid ARM64 preflight` 和 JSON 顶层的 `rudroid` 给出同一次扫描的原生路线判断。`rudroid.route` 为 `mono-arm64` 时可继续准备桌面 Mono 数据，为 `apk` 时可准备已识别的 ARM64 Android IL2CPP 包。值为 `null` 时看 `reasons`。顶层 `route` 保留制包工具的模板建议，命令退出码沿用该工具的规则。

`player_profile_candidate` 指出可试用的 Player 配置，`reference_title_same_engine_and_backend` 指出同一 Unity 版本和脚本后端已有真机记录的参考标题。`requirements` 列出制备和上机待核对项。`pinvoke_modules` 列出声明外部调用的程序集，`native_plugins` 列出发现的插件；按实际调用核对 ARM64 库或显式映射。依赖清单为 `null` 表示缺少调查数据，`unreadable_assemblies` 非空表示程序集调查不完整。

### 来源游戏、路线与掌机运行时

`new --route` 选择制包模板。Rudroid 的掌机运行入口是 `unityloader <config.toml>`，已验证的输入和设备列在最后一栏。

| 来源游戏 | `new --route` | 制备和运行方式 | Rudroid 真机验证范围 |
|---|---|---|---|
| A · Linux / macOS 桌面 Mono | `android` | 转换游戏数据，使用 ARMv7 Android Player。掌机固件需要 32 位用户态。 | ARM32 路径尚未验收。 |
| B · 仅 Windows 桌面 Mono | `android` | 与 A 共用 ARMv7 模板。受支持的 D3D11 着色器需转为 GLES，Unity 2021.2 及以后的构建还需匹配的 Unix 类库。 | 尚无此路线的真机验收。 |
| C · 含完整托管程序集的桌面 Mono | `il2cpp-arm64` | 使用匹配的 Unity 工具链，在制作者电脑或掌机安装阶段编译为 ARM64 IL2CPP。 | 由此路线生成的 Player、metadata、原生插件及游戏流程仍待验收。 |
| D · 游戏自带 Android APK / XAPK / AAB | `apk` | 整理 base、split、ABI 库和资源。对于已制备的 ARM64 IL2CPP 游戏，`unityloader` 加载配套 Player、游戏原生库和 metadata。 | [按游戏构建和设备记录的原生验收结果](../crates/docs/ACCEPTANCE.md)。其他 ABI 与容器入口分别验证。 |
| E · Linux x86-64 Mono 或 IL2CPP | `box64` | Box64 转译原版 Linux Player。 | 外部执行路线，单独验证。 |
| F · 桌面 Mono，目标为 ARM64 Mono JIT | `mono-arm64` | Rudroid 可用 `prepare` 制备数据，保留游戏 DLL。掌机用 `unityloader` 启用 `unity_mono`，加载匹配的 Player、桥和 JIT。 | KTC 的 Linux 构建已在 MiniLoong 验证首岛骑行、音频、存档和读档。[原生 Mono 配置](../crates/docs/UNITY-MONO.md) |

### 将桌面 Mono 游戏制备为 ARM64 运行数据

`prepare` 把资源转换和 Android 数据打包合为一个主机步骤。它接收原始 `<Game>_Data`、匹配该 Unity 版本的 ARM64 Android Player 目录、上游转换 JSON 和一个新的输出目录。已有 Player 时直接运行第二条命令。

```sh
python tools/unity_port.py player fetch "$UNITY_VERSION" "$PLAYER" \
  --backend il2cpp --abi arm64-v8a --managed --installer "$INSTALLER"
python tools/unity_port.py prepare --src "$GAME_DATA" --pkg "$PLAYER" \
  --config "$CONVERT_CONFIG" --steps shaders,settings,audio,quality \
  --name MyGame --package com.pmunity.mygame --out "$ANDROID_OUT"
```

`--steps` 和 `--set` 由标题配方决定。若游戏需要替换个别程序集，先准备好对应 DLL，再重复传入 `--managed-file`。转制工具在工作副本上运行 `convert`，然后执行 `android-data --mono64 --pack all`。输出的 `zip/android_package.json` 描述 APK 和可能生成的 OBB，供下一步配置运行时使用。

每次制备使用新的工作副本。上游的转换进度记录按步骤名和文件路径续跑，更换源文件或配方后沿用旧记录会跳过需要重做的转换。保留转换日志中的 shader、原生插件等兼容性提示，按游戏场景验收画面与行为。

### 为 ARM64 Mono 端口选择 Rudroid loader

在原生运行配置的 `[plugins].enabled` 中加入 `unity_mono`，在 `[unity.mono]` 中指定匹配的桥、JIT、Managed 目录和清单。完整配置见[原生 Mono Runtime](../crates/docs/UNITY-MONO.md)。

```sh
unityloader "$CONFIG_TOML" --check
unityloader "$CONFIG_TOML"
```

另需制作上游格式的发行包时，`routes` 列出模板，`new --route` 生成端口目录，`package` 生成发行包。各模板的输入和依赖见[上游制包说明](https://github.com/wild1walker/portmaster-unity/blob/7f617e282953f8981ca4a38537277a8a9983ff90/docs/porting.md)。

### ARM64 Mono 线程回收对照

PortMaster-Unity 固定版本的 Unity 6000.0 ARM64 Mono 构建使用 8 字节 Boehm granule。
出现 `GC_destroy_thread_local` → `GC_unregister_my_thread_inner` 段错误时，可用
[关闭线程局部分配的构建补丁](patches/pmunity-mono64-no-thread-local-alloc.patch)进行对照。
在同一 Mono、BDWGC 和辅助库的 ARM64 容器测试中，默认配置在 128 次托管定时器回调和
多次显式 GC 后，于运行时关闭时崩溃。关闭线程局部分配后，同一测试连续五次完成。
补丁保留 8 字节 granule、Boehm GC 和线程支持。实际分配开销、帧率与内存仍需按标题和设备验证。

在 Rudroid 根目录运行。`PMUNITY_MONO` 指向固定提交的单独源码副本，`MONO_WORK` 指向
专用主机构建目录。先试应用补丁，再构建。补丁使用独立 Docker 卷隔离两种配置的构建缓存。

```sh
patch --dry-run -d "$PMUNITY_MONO" -p1 < tools/patches/pmunity-mono64-no-thread-local-alloc.patch
patch -d "$PMUNITY_MONO" -p1 < tools/patches/pmunity-mono64-no-thread-local-alloc.patch
MONO_REF=e3f5e86f047452d5ae757c195490df0059418621 \
  "$PMUNITY_MONO/tools/mono64/build-mono.sh" 6000.0 "$MONO_WORK"
python tools/unity_port.py package "$PORT" --donors "$DONORS" --mono64-work "$MONO_WORK"
```

构建后检查 `$MONO_WORK/out/unity-6000.0-mbe/SOURCE.txt` 和 `libmonobdwgc-2.0.so`。
上游打包器从该构建目录读取 Mono 和许可证，Mono bridge 与其余宿主组件沿用固定版本的标准构建。
