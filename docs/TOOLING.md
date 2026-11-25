# 工具与调试入口登记

## 逻辑

Rudroid 的可执行入口分三类，**先查本表再动手**；禁止为同一目的平行发明第二套脚本、第二套 Docker、第二套 Python 环境或未登记的“临时工具”。

| 类 | 目录 / 落点 | 谁写用法细节 |
|---|---|---|
| 离线主机工具 | `tools/` | [tools/README](../tools/README.md)、[tools/AGENTS](../tools/AGENTS.md) |
| 构建 / 交付 / ABI | `crates/scripts/` | [BUILD-DOCKER](../BUILD-DOCKER.md)、[crates/AGENTS · Contents](../crates/AGENTS.md) |
| 运行时调试与探测 | loader CLI、环境变量、可选 kit feature | 本表 + [RUNTIME-POLICY](../crates/docs/RUNTIME-POLICY.md) / 相关 AGENTS |

规则：

1. **复用优先**：已有入口能扩参数或加子命令，就不新建顶层脚本。
2. **登记同步**：新增、改名、弃用、删除入口时，同一变更更新本表；`tools/` 与 `crates/scripts/` 的 AGENTS/Contents 一并改。
3. **过时即删**：无调用方、被固定入口取代、或只服务已结束实验的脚本/补丁，删除实现并从表中移除，不留“也许有用”的平行副本。
4. **任务垃圾不进仓库契约**：`.scratch/<component>/` 只承载当前任务输出；不把其中的 harness/validate 脚本提升为正式工具，任务结束按 [AGENTS 工作区约定](../AGENTS.md) 清理。
5. **命令全文**仍只写在 BUILD-DOCKER / tools/README；本页负责“有什么、何时用、别另起炉灶”。

## 唯一现行构建入口

| 入口 | 用途 | 不要做的事 |
|---|---|---|
| `bash crates/scripts/docker-rust.sh {build\|test\|check} …` | 唯一 Docker 内 Rust 构建/检查/测试 | 不用本机随意 cargo、不另建镜像/容器/volume、不全局 prune |
| `crates/scripts/rudroid-rust-gles.Dockerfile` | 固定镜像配方（维护者授权后偶发重建） | 不改配方冒充日常入口 |
| `crates/scripts/build-kit.sh` | 由 docker 入口调用；写入 `RUDROID_KIT_FEATURES` 指纹 | 不绕过 docker-rust 直接当日常入口 |
| `crates/scripts/vulkan-kit.sh` | `--vulkan` / `--portable-wayland` 时：Cargo kit + 可选 WSI/客户端后处理 | 不单独当 GLES 构建入口 |
| `crates/scripts/malisx2-kit.sh` | `--malisx2` 后处理：独立 Linux ICD 包 | 不装系统驱动；Android/Bionic 包不可替代 |

可选构建开关（均经 `docker-rust.sh`，细节见 BUILD-DOCKER）：`--sdl`、`--vulkan`、`--portable-wayland`、`--malisx2 [--driver-only]`、`--thin-lto`、`--pgo-generate` / `--pgo-use`、`--diag-probes`、`--release` / `--debug`。

非现行：`crates/scripts/cross-build-linux.sh` 为历史跨编译入口；**日常与交付只用 docker-rust**。若需保留仅作对照，勿新增依赖它的流程；确认无维护价值后删除并更新本表。

## 交付与 ABI 审计

| 入口 | 用途 |
|---|---|
| `crates/scripts/deploy-select.py` | 从完整 kit 拷贝标题成员子集 + loader；拒绝测试/训练标记 |
| `crates/scripts/kit-manifest.py` | 扫描/校验 `runtime_build_id`；拒混套与测试成员 |
| `crates/scripts/kit_markers.py` | 测试/训练导出标记表（与 loader 门禁同步） |
| `crates/scripts/linux_abi.py` | 静态 ELF GLIBC/架构审计（默认上限 2.28） |
| `crates/deploy/*.txt` | 已登记 Deploy Selection 成员表 |

对应 unittest：`test_deploy_select.py`、`test_linux_abi.py`（及 BUILD-DOCKER 所列）。生产顺序只走 BUILD-DOCKER「生产部署顺序」。

## 可选原生输入（仅特殊 kit）

| 入口 | 何时 |
|---|---|
| `vulkan-input.py` | `--vulkan`：校验 WSI/Headers 等固定源 |
| `wayland-input.py` | `--portable-wayland`：私有 SDL + 通用 Vulkan loader 输入 |
| `malisx2-input.py`（+ `malisx2-c11-label.patch`） | `--malisx2`：ICD 源校验/打包 |
| `pgo-input.py` | `--pgo-use`：不可变 profdata 快照 |

各有配对 `test_*-input.py`。普通 `--sdl` 不调用这些。

## 启动器 / 运行时辅助

| 入口 | 用途 |
|---|---|
| `rudroid-unity-loader` / 掌机名 `unityloader` | 正式启动；`--check` 只验 kit 拓扑（+ 通用 config）；run 才跑引擎校验 |
| `--graphics-features` | 查询运行库图形能力 token（不启动游戏） |
| `--probe-vulkan` | 有界原生 Vulkan 预检（不加载游戏、不报游戏帧） |
| `crates/scripts/guest-exec-view.py` | 启动器显式选用：noexec 介质上的可执行库视图 + 有界网络命名空间；无资源拷贝、无常驻监督 |
| `RUDROID_DIAGNOSTICS=1` | 打开普通诊断日志；默认安静，错误/致命仍可见 |
| `RUDROID_SKIP_GUEST=1` | run 降级为插件/phase 烟测；不算启动验收 |
| kit feature `diag-probes`（`--diag-probes` 构建） | 几何/Wayland interval/帧捕获等私有探针；生产 kit 不要带；须同时 `RUDROID_DIAGNOSTICS=1` |

策略与门禁见 RUNTIME-POLICY；Mono 激活见 [UNITY-MONO](../crates/docs/UNITY-MONO.md)。

## 离线主机工具（`tools/`）

| 入口 | 用途 | 角色 |
|---|---|---|
| `unity_assets.py` | 一次生成完整资源树（默认 `astc-1g`） | **用户**现行 |
| `unity_port.py` | 固定 PortMaster-Unity：`check` / `prepare` 等 | **用户**现行 |
| `pmunity_rudroid/` | Rudroid 对上游 check/prepare 的扩展 | 内部，经 `unity_port.py` |
| `unity_astc/` | ASTC 转换实现 | 内部库 |
| `unity_astc/repair_mips.py` | 定点 mip 尾层修复 | **开发者**只读/定点，非用户步骤 |
| `audit_game_assets.py`、`audit_pck.py` | 只读资源/PCK 审计 | **开发者** |
| `patches/pmunity-mono64-no-thread-local-alloc.patch` | 可选上游 Boehm 线程退出 A/B | 显式选用，非默认 |
| `tools/tests/`、`unity_astc/tests/` | Python 回归 | `python -m unittest discover -s …` |

用法与约束以 [tools/README](../tools/README.md) 为准；不要再挂平行的“资源转换 CLI”。

## 测试支撑（非独立产品 CLI）

| 落点 | 用途 |
|---|---|
| `crates/test-support/` | 嵌套 cargo、`--locked`、fixture C 编译、日志捕获；测试 dlopen cdylib 须经此重建 |
| `crates/scripts/test_*.py` | 脚本层回归（不经 Docker cargo） |
| crate 内 `tests/`、`tests/unit/`、`tests/fixtures/` | Rust/guest 探针；命名与忽略原因须诚实 |

新增测试辅助优先放 `test-support` 或既有 `scripts/test_*.py` 模式，不散落未登记的 ad hoc runner。

## 变更检查清单

- [ ] 本表已增删改对应行  
- [ ] `tools/AGENTS` 或 `crates/AGENTS` Contents 已同步  
- [ ] 用户步骤只出现在 BUILD-DOCKER 或 tools/README（本表不堆命令全文）  
- [ ] 无第二套同名用途的脚本留在树内或 `.scratch` 被误当成正式入口  
- [ ] 弃用入口已删除或在本表标为非现行，并去掉文档/CI/启动器引用  
