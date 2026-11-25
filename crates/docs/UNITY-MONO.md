# 原生 ARM64 Unity Mono

## 逻辑

桌面 Mono 标题走**同一套** host / Android Player / SDL 路径；差别只在脚本后端：
用 Linux ARM64 Mono JIT + PortMaster-Unity 桥（对外仍叫 `libil2cpp.so`）代替客体
IL2CPP，而不是另起一套 loader。

激活条件（必须同时满足，缺一不切 Mono 计划）：

1. kit 部署可选 Runtime `unity_mono`（`libunity_mono.so`）；
2. Host Config 存在 `[unity.mono]`（路径与清单字段齐全）；
3. 不与 `il2cpp_patch` 同 kit。

仅部署能力、没有配置段 → 普通 IL2CPP 计划不变。配置段在、能力缺失 →
`validate_config` 拒绝。能力注册命名服务 `rudroid.unity.mono.v1`；真正开桥在
Unity `prepare` 的 `start_bridge`（无配置则为 no-op）。

编排与回调契约见 [ENGINE-PROFILE-CONTRACT](ENGINE-PROFILE-CONTRACT.md)；
能力内设计见 [mono AGENTS](../capability/unity/mono/AGENTS.md)；
player 侧调用见 [player_lifecycle AGENTS](../capability/unity/player_lifecycle/AGENTS.md)。
数据制备见
[主机工具 · 桌面 Mono](../../tools/README.md#将桌面-mono-游戏制备为-arm64-运行数据)。

## 怎么启动

| 形态 | 用法 |
|---|---|
| 掌机启动器（二进制名 `unityloader`） | `unityloader <config.toml>`；kit 在旁路 `rudroid.runtime/` |
| 直接调用构建产物 | `rudroid-unity-loader <deploy_dir> --config <config.toml>` |

`--check` 只验证 kit descriptor/拓扑，以及（若提供了 Host Config）通用
render/memory。**不**跑引擎 `validate_config`、JNI、Mono 清单/路径或 guest。
完整 Mono 校验只在 run：register-JNI 后的 `validate_config`，再在 prepare 里
`start_bridge`。

成员表与部署审计：
[unity6-mono.txt](../deploy/unity6-mono.txt)、
[BUILD-DOCKER · 生产部署](../../BUILD-DOCKER.md#生产部署顺序)。

## 配置与布局

在现有 Host Config 上增加 `[unity.mono]`，并保留该标题所需的 Player profile、
Android capability、资源与显示配置。字段语义与校验统一走
`rudroid_unity_common::mono_config`（`mono_config_from_api` /
`mono_section_present_in_api` 通过 `rudroid.host.config_toml.v1` 直接解析同一份权威 TOML，
确保未知字段与半截 `[unity.mono]` 节行为一致）。

```toml
[paths]
game_files = "/port/game"
android_package_code = "/port/data/Title.apk"
android_obb_dirs = ["/port/data"]

[package]
packageName = "com.example.title"
versionCode = 1

[unity.mono]
bridge = "/port/native/libil2cpp.so"
runtime = "/port/native/libmonobdwgc-2.0.so"
managed_dir = "/port/managed"
manifest = "/port/data/android_package.json"
unity_version = "6000.0.66f2"
# abi = "arm64-v8a"
# plugins_map = "/port/data/plugins.map"
```

宏观约束：

- `bridge` / `runtime` / `managed_dir` / `manifest` / APK / 显式 `plugins_map` 用绝对路径；清单的 package、version、unity、abi、`scripting=mono`、apk 须与配置一致；ABI 目前为 `arm64-v8a`。
- 客体：`game_files/lib/arm64-v8a/` 放 `libmain.so`、`libunity.so`。桥可与客体分目录；分目录时可省略客体 `libil2cpp.so`，由能力按 `bridge` 注册精确别名。若客体里**已有**该文件，必须与配置桥为同一文件，否则拒绝。
- JIT 与桥同目录（`libmonobdwgc-2.0.so`），并带上该 Mono 构建的 `libMonoPosixHelper.so`、`libmono-native.so`。单目录布局允许桥目录与客体 `lib/arm64-v8a/` 相同。`UA_MONO64_*` 环境覆盖一律拒绝。
- `[plugins].enabled` 含 `unity_mono`，且与 kit 同 Release 身份。

路径别名与 guest 识别细节见 `android_elf_load` 与 mono `start` 实现。

## 帧率与插件（策略层）

- **帧率**：`[unity.mono].frame_pacing` 默认 `android`。`desktop` 需核定
  `display_refresh_hz`（24–360），按引擎 `vSyncCount` 与游戏
  `Application.targetFrameRate` 算目标；托管 getter 仍返回游戏自己的请求。
  `[unity].target_fps` 只管宿主循环。`observe` 只观察不改写。诊断采样见
  `RUDROID_DIAGNOSTICS=1`；实现见 mono `pacing.rs`。
- **P/Invoke 映射**：显式 `plugins_map` 优先，否则清单旁 `plugins.map`。
  支持 `lib` / `guest` / `native` / `none` / `stub steam`；不开放任意宿主路径。
  单目录下 `lib` 按 ELF `DT_NEEDED`（Bionic `libc.so` vs GNU `libc.so.6`）归类。
  细则见 mono `steam.rs` 与桥侧 dllmap。

## 交付边界

`unity_mono` 是同 kit 可选 Runtime；桥与 JIT 进程内固定持有至退出。
Player、桥、JIT、辅助库、游戏程序集由制作者分别提供并核定许可与 GLIBC。
例：核心 kit 直接依赖上限 GLIBC 2.28；某 KTC 桥/JIT 更高——两套下限按实际 ELF
与固件分别审计。引擎 Build 字符串仍可能写 Android IL2CPP；脚本路径以 Mono
初始化与实装模块为准。IL2CPP 专用 hook / 标题补丁不进 Mono 配置。

新端口按 [ACCEPTANCE](ACCEPTANCE.md) 逐项记加载、画面、读档、输入与帧率。
