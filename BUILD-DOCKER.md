# Rudroid Docker build and test

开发顺序、插件边界、生命周期 review 和已知坑见 [开发规范](DEVELOPMENT.md)。
新设备/游戏的能力复用、预检与分阶段验收见 [掌机游戏移植执行指南](crates/docs/HANDHELD-PORTING-WORKFLOW.md)。本文构建入口仍针对当前 ARM64 Rust kit，其他 ABI/引擎的调查范围不代表已有对应工具链。

## 构建与定向测试

```sh
bash crates/scripts/docker-rust.sh check
bash crates/scripts/docker-rust.sh build --release --sdl
bash crates/scripts/docker-rust.sh test
bash crates/scripts/docker-rust.sh test --sdl -- -p rudroid-unity-loader
```

`--` 之后的参数原样追加给 cargo，只用于收窄 check/test（如 `-p <crate>`、
`--test <name>`）；带 `-p` 时不再加 `--workspace`，build 始终是整个 kit，
不接受 `--` 参数。

实验 Vulkan kit 使用 `bash crates/scripts/docker-rust.sh build --release --vulkan`。
它包含 SDL/Vulkan feature，先完整构建 Rust，再经 native-build 生成独立
`target/release/vulkan-wsi/`。固定镜像已有 GNU 编译器和 wayland-scanner 即可；
WSI、Vulkan-Headers、Wayland protocols 与 UAPI 头文件按固定提交/SHA 校验到
既有 native-inputs 缓存，不安装工具链、不替换系统驱动。附带上游许可和来源清单，
链接的自有 Rust allocator 遵循根非商业 LICENSE，打包时一并附带正文及 NOTICE。
该选项暂不与 PGO 混用，定向 test 必须包含
`-p rudroid-linux-sdl-runtime`。普通 `--sdl` 不下载、构建或加载这些依赖。
WSI 不属于 capability，需要单独做 ABI/内容审计，并通过游戏私有环境选择；
配置及当前范围见 [运行策略](crates/docs/RUNTIME-POLICY.md#可选-vulkan-后端)。
任何对外二进制交付都要附带根 `LICENSE`、`NOTICE`、`THIRD-PARTY-NOTICES`
及精确对应版本的公开源码地址；可选 WSI 的许可目录也要完整保留。
SDL/Vulkan 全量回归使用 `test --release --vulkan`；Debug 的大型 fixture 副本
可能填满固定的 1 GiB 临时目录，Debug 保留用于定向回归，不扩大容器限额。

## 固定环境

仅复用已有本地 `rudroid-rust-gles:20260917`（linux/arm64），不会拉取或构建镜像。
镜像缺失时停止，工具链重建需用户明确授权，不自动补建或覆盖已有镜像。
[镜像配方](crates/scripts/rudroid-rust-gles.Dockerfile) 已改为固定 digest 的官方
Rust 1.88/Bullseye 公共基础镜像，移除私有 review 镜像依赖，沿用验收环境的
编译器和 SDL 包安装步骤。本机镜像层核对确认该公共基础是验收镜像的精确祖先；
尚未执行干净环境镜像重建，不能把祖先核对当作完整复现验收。

首次准备环境时，由维护者明确授权后执行一次：

```sh
docker build --platform linux/arm64 \
  -f crates/scripts/rudroid-rust-gles.Dockerfile \
  -t rudroid-rust-gles:20260917 .
```

已有镜像继续按固定入口构建；公开首发仍需验证上述首次准备流程。

容器名固定为 `rudroid-rust`，使用 `--rm`、只读根文件系统、2 CPU、4 GiB
内存和最大 1 GiB 临时目录。同名运行互斥：已有运行中的同名容器时每 5 秒轮询排队，
直到现有任务结束再启动；`docker run` 抢名失败（退出 125）也回到排队，不换名。
残留的已停止同名容器不会被等待，入口以 125 退出并提示核查后 `docker rm`。
源码只读挂载，Cargo 缓存和产物复用 `crates/target/docker-linux-arm64/`；
产物位于其 `target/debug` 或 `target/release`。增量缓存关闭，但普通编译缓存保留。
固定目录避免多份缓存，不代表磁盘硬配额；仍需观察该目录占用。

`android_bionic` 的 ARM64 GNU stdio/POSIX 变参与 layout gate、ioctl opcode 尾调用边界，以及 jni_vm/zlib/media_fallback/native_events 的 C 边界，统一经 `crates/native-build` 使用镜像已有 gcc/ar、固定参数
（release `-O2`、其他 `-O0`）编译私有 PIC archive 或只编译的 ABI gate；不读取 CC/CFLAGS 覆盖，也不安装新的依赖。跨编译
对应使用已有 aarch64-linux-gnu-gcc/ar。Rust 与 C/header/`.S` 源码及 build.rs
均纳入 kit 指纹，修改 C 桥接也必须重建 loader 与插件。

不得再运行 `.scratch` 的历史 Docker 创建脚本，或按日期/ticket/agent/worktree
创建镜像、容器、volume、builder、target。检查空间用 `docker system df` 和
`du -sh crates/target/docker-linux-arm64`；清理时列明对象，禁止全局 prune。

Rust 回归验证可执行 `bash crates/scripts/docker-rust.sh test --sdl`（或追加
`--release`）。入口在同一个容器中使用已有 Xvfb 和 Mesa 软件 GLES，不创建
其他镜像或容器。Xvfb 使用 `-noreset`，避免最后一个 SDL 客户端关闭时重置显示服务器，影响同一测试进程随后的窗口重建；选项语义见 [Xserver 手册](https://xorg.freedesktop.org/archive/X11R7.0/doc/html/Xserver.1.html)。这只影响测试显示环境，不修改生产运行库或掌机驱动。
Mesa shader cache 复用现有 target 可写挂载下的 `mesa-shader-cache/`，设置 32M driver 上限；不落到只读 `/root/.cache`，不新增 volume。driver 上限不是整个目录/RSS 硬配额；统计 cold/warm 性能时应记录 cache 状态。嵌套测试构建统一经 `rudroid-test-support` 的 cargo
helper：`--locked`、当前 profile，feature 只取 `RUDROID_KIT_FEATURES`（入口
随 `--sdl` 导出）；外层实际 feature 与该变量不一致时测试直接失败。无 SDL、
test-fixtures 等变体编到 `target/variants/<name>/`，不覆盖 kit 产物。独立 fixture
的 Cargo.lock 一并维护，`--locked` 确保测试不修改只读源码。guest C 探针
（rudroid52/53/56/79）由测试用容器内 gcc 编译，不再依赖 RUDROID5x_* 环境变量；
无 SDL 运行中需显示的测试显示为 `ignored` 并给出原因。

Python工具回归可按受影响范围运行：

```sh
python3 crates/scripts/test_deploy_select.py
python3 crates/scripts/test_linux_abi.py
python3 crates/scripts/test_guest_exec_view.py
python3 crates/scripts/test_pgo_input.py
```

Rust 的 Linux ABI 基线独立于只读 C++ 参考项目的 builder 验收。当前 Rust 生产 kit 的
直接版本需求最高为 **GLIBC_2.28**（包括 weak statx 的版本节点），并不因为
Rust 官方 target 支持 glibc 2.17 就能宣称同一产物在 2.17 上运行。
`deploy-select.py` 在修改目标目录前检查 Linux ELF 的 ARM64 GNU ABI，默认
`--max-glibc 2.28`，要求更高的产物会非零拒绝；更旧固件可以指定
`--max-glibc 2.17`，它只检查、不会降级 ELF。Mach-O 部署保持独立流程。

以下示例路径先替换为实际 kit、报告文件和取回的目标 SDL 库：

```sh
python3 crates/scripts/linux_abi.py --dir "/path/to/kit" --max-glibc 2.28 --output "/path/to/abi-report.json"
python3 crates/scripts/linux_abi.py --file "/path/to/target/libSDL2-2.0.so.0" --max-glibc 2.28
```

审计静态读取 PT_INTERP、DT_NEEDED 与 GNU version/import 表，不运行 ELF/ldd；
所给文件的直接需求通过后，仍需核验目标固件 SDL/EGL/GLES/libgcc/驱动的
传递依赖和运行表现。工具链与旧 sysroot 的调整继续遵守固定 Docker 契约，
不在掌机直接替换 libc。完整边界见 [Rust Linux 兼容说明](crates/docs/LINUX-COMPATIBILITY.md)。

## 生产部署顺序

**测试产物不能直接当作生产 kit。** `cargo test` 的 dev-dependency 可能合并测试 feature，生成带测试导出的 `.so`，且其 kit id 可能仍相同。部署前执行 `bash crates/scripts/docker-rust.sh build --release --sdl`；随后 loader、deploy-select 和 kit-manifest 按 `scripts/kit_markers.py` 拒绝测试/训练成员。

下面以已有 Silksong 成员名单演示打包，产物放在已有 target 下；选择名单不代表该游戏已通过实机验收。`--max-glibc` 应按交付目标和项目兼容上限确定，此例沿用当前 2.28 上限。build/check/test都从仓库根运行。

```sh
rudroid_build_dir=crates/target/docker-linux-arm64/target/release
rudroid_kit_dir=crates/target/docker-linux-arm64/target/deploy-silksong
rudroid_manifest=crates/target/docker-linux-arm64/kit-manifest-production.json

python3 crates/scripts/kit-manifest.py scan \
  --build "$rudroid_build_dir" --output "$rudroid_manifest"

python3 crates/scripts/deploy-select.py \
  --build "$rudroid_build_dir" --dest "$rudroid_kit_dir" \
  --from-profile crates/deploy/silksong.txt --include-loader --max-glibc 2.28

python3 crates/scripts/kit-manifest.py verify \
  --dir "$rudroid_kit_dir" --manifest "$rudroid_manifest"

python3 crates/scripts/linux_abi.py \
  --dir "$rudroid_kit_dir" --max-glibc 2.28 \
  --output "$rudroid_kit_dir/linux-abi.json"
```

必须保留源码 revision 或源码清单、kit id/features、成员名单、配置和审计结果。loader 与所有部署插件来自同一套完整构建，不只替换单个旧 `.so`。`kit-manifest verify` 检查在场成员的构建身份并允许子集部署，不能替代成员名单完整性、游戏配置或文件内容校验；是否满足实际依赖还要由部署选择和 loader 验证。

Cargo 不会删除已移除目标或测试 fixture 留下的顶层 `.so`。扫描出现混套或额外成员时，
先按当前 workspace 的生产目标核对；只将确认过时的产物移入回收站，再重新扫描。
同 kit id 也不能把 fixture 变成生产成员，不整目录盲目打包，不关闭混套/测试标记门禁。

不要向正在运行 loader 的目录重新部署。部署脚本的逐文件原子替换不构成整套目录同时切换；先停止该运行，再更新成员与配置。

若启动器使用 `unityloader`，在完成上述门禁后按该启动器布局重命名 loader 并记录最终文件 SHA；deploy-select 本身输出名仍为 `rudroid-unity-loader`。`reply_catalog` 则由部署脚本显式改为 `libreply_catalog.so`，其余能力使用 `lib<id>.so`。不能让配置 allowlist 与部署文件 stem 不一致。

## 验证层级

| 检查 | 能证明什么 | 不能代替什么 |
|---|---|---|
| linux_abi / manifest / test-marker | 静态架构、依赖上限、构建身份及测试导出门禁 | 完整成员与配置、传递依赖、实际运行 |
| loader `--check` | dlopen 可信能力，检查 descriptor 与拓扑 | 已执行原生构造函数，不是纯静态沙箱；不运行 phase、JNI 注册或 guest |
| `RUDROID_SKIP_GUEST=1` | 已实现的宿主装配路径；在窗口创建前返回 | Unity初始化、窗口、声音、游戏画面 |
| 单元/接口/生命周期测试 | 对应断言和native边界 | 全部游戏、固件GPU或物理外设 |
| 实际游戏验收 | 已记录的加载→引擎→画面→输入/音频→存档→退出 | 未测游戏/场景、长期泄漏或公平性能收益 |

收窄测试示例：

```sh
bash crates/scripts/docker-rust.sh test --release -- \
  -p rudroid-common --test unsafe_boundaries
```

纯文档修改检查链接与格式即可。源码变更按受影响模块收窄回归；候选固定后统一
执行必要的跨标题回归，不为每个小改动重跑全部游戏。当前交付的结果与明确ignored
分类见[验收](crates/docs/ACCEPTANCE.md)，不能用默认跳过掩盖缺环境。

## 本地真游戏输入

普通 workspace 编译与测试不依赖 Rudroid 目录。显式选择真游戏测试时，
提供 Vampire Survivors 1.14 的原始 game-data 目录，包含 `base.apk`、
`split_UnityDataAssetPack.apk` 和 `lib/arm64-v8a/{libmain,libunity,libil2cpp}.so`：

```sh
RUDROID_VAMPIRE_GAME_DIR=/absolute/path/to/gamedata \
  bash crates/scripts/docker-rust.sh test --release --sdl -- \
  -p rudroid-unity-loader --test real_game_startup \
  real_vampire_survivors_standard_memory_chain -- \
  --exact --ignored --nocapture --test-threads=1
```

入口将该目录只读挂载到同一固定容器的 `/game-data/vampire-survivors`，
不复制原始游戏包到源码、不修改原始存档。未提供变量时仍支持仓库本地
`dist/chuimi-vs-install-20260910/staging-verified/vampiresurvivors114/vampiresurvivors114/gamedata`
布局。显式真游戏测试缺少输入时失败，不静默跳过。测试使用临时会话和
独立存档，证据保存在既有 target 目录；仍不能替代其他游戏或掌机验收。

## 可选 ThinLTO 编译对照

固定入口支持 `bash crates/scripts/docker-rust.sh build --release --thin-lto --sdl`；
相关 test/check 同样支持 `--thin-lto`，必须与 `--release` 同用。入口只向固定容器
传入 `CARGO_PROFILE_RELEASE_LTO=thin`，嵌套测试构建继承同一设置，不另建缓存或环境。
它只改变宿主 Rust 链接优化，不重编闭源 Unity/IL2CPP/驱动；游戏画质、线程和内存
档位仍独立。普通 Release 保持默认；[MiniLoong/VS首波回切](crates/docs/PERFORMANCE-AUDIT.md#编译与诊断实验)未见净收益，其他设备/标题仍需独立验收。

两种编译使用相同 release 输出目录；切换前先按生产流程冻结各自的完整同 kit
产物。LTO 环境值进入构建身份并触发重新构建，禁止混用；测试后仍需重新纯生产
build 和 ABI/test-marker 审计。不要用省下的文件体积直接宣称省下相同的物理内存。

## 可选 PGO 训练与反馈编译

普通 Release 仍为默认。PGO 是独立实验选项，要求 `--release`，不改游戏分辨率、
帧率上限或内存档位。[Rust 官方流程](https://doc.rust-lang.org/rustc/profile-guided-optimization.html)
要求生成插桩产物、运行代表场景、合并原始 profile、再反馈编译；插桩版本的速度不能
当作优化后速度。固定入口支持：

```sh
bash crates/scripts/docker-rust.sh build --release --sdl --pgo-generate
# 在已核验兼容性的私有测试目录运行训练 kit，并为每次训练设置独立空目录：
# LLVM_PROFILE_FILE=/path/to/private-run/profiles/%p-%m.profraw
# 用已经存在且经实际格式验证的 llvm-profdata 合并该次训练的 *.profraw。
bash crates/scripts/docker-rust.sh build --release --sdl --pgo-use /absolute/path/merged.profdata
```

两者只使用原固定镜像/容器/缓存/资源约束，额外显式设置
`CARGO_BUILD_TARGET=aarch64-unknown-linux-gnu`，让插桩 flags 不影响宿主 build script。
产物位于同一缓存内的 `target/aarch64-unknown-linux-gnu/release/`，区别于普通
`target/release/`；复制/审计时必须指向本次实际输出。嵌套 Cargo 继承目标，
test-support 对 workspace、fixture、variant 的查找也加入该目标层，避免拿到旧宿主产物。

common 根据真实 `profile-generate` 编译 flags 选择内部训练导出，不改变 Cargo
feature 图。CLI 仍在成功完成 Unity、音频 worker 和服务退出后走 `_exit`，仅训练版
在此前为每个已验证并固定的 DSO 及 loader 写出各自 LLVM counters；失败以非零退出，
不改用 host atexit。该接口没有 guest 指针、共享跨 DSO static、后台线程或帧钩子，
普通/反馈版不包含 collector 导出或采样工作。合并profile只用于编译，不打包常驻游戏；反馈后代码布局仍可能更大或更慢，需要实测。`rudroid_pgo_training_flush` 是非生产
标记，正常 loader、deploy-select、kit-manifest 会拒绝训练成员，不能把它当发布候选。
训练 kit 的依赖也要独立审计；本机固定编译器生成版含 GLIBC2.29 weak import，
只在已核验的 GLIBC2.38 私有设备目录试验，不能沿用生产的2.28兼容声明。

`--pgo-use` 只接收非空、最多64MiB的普通文件，先冻结到现有 target 的
`pgo/inputs/<SHA256>.profdata`，再将该文件只读挂载到内容命名的编译路径。
路径进入 `RUSTFLAGS` 和 kit 身份，同一路径输入换内容也会得到新身份；已有冻结文件
损坏会拒绝。LLVM 校验 profile 格式，反馈编译启用 missing-function 报告；不要把
只有参数检查的 `cargo check` 当作反馈代码生成成功。不同版本 llvm-profdata 未必
兼容原始格式，必须实际合并再用固定编译器完整 build 验证，不安装/替换工具链。

每次训练保留源码身份、原始 profile、合并工具版本、成员和 profile SHA、场景截图、
输入时间线及退出码。不要混入此前源码或模拟场景数据。反馈版重新完成同 kit、
test-marker、GLIBC2.28审计后，才能与同源码普通 Release 比较；缺失场景/函数的
训练覆盖不等于通用优化，也不能用单一标题结果给所有设备默认开启。
