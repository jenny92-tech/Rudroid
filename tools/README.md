# 一次生成 Unity 游戏资源

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
未知布局的资源包逐字节保留，并记入结果；未知纹理格式保留原载荷，不解码或重写。
不尝试猜布局，也不宣称这些资源已优化。
流式纹理保持流式存放，保留原 `.assets` 的对象位置；内联或外部纹理布局保持原样。

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
