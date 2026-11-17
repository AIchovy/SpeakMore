# macOS 原生界面与五分钟处理资源检查

本检查补齐 WebView/IPC 和进程归属层。使用独立的 Model Check 应用、公开英语样本和真实打包 worker；结果不能替代真实麦克风、最低机型、自然语料质量或商业发布验收。

## 明确的测试构建

```sh
CARGO_NET_OFFLINE=true node scripts/build-model-check.mjs --public-audio
```

它使用既有的 `com.speakmore.modelcheck.20260916` 独立数据目录和已验证的测试签名清单，不接触普通用户数据库。窗口标题为 `SpeakMore · Public audio fixture · No microphone`，页面持续显示公开音频测试提示。新增 `model-audio-check` feature 只替换采集源、设备探测和采集权限状态；没有隐藏 IPC、动态音频路径或伪造 ASR 返回值。构建标识和窗口标题不匹配时拒绝启动。

输入为已缓存公开 WAV，SHA-256 `eb1eb008904465b74c304aad8342e8c7d3c6e61ffe9f66adcaca9cf0f76a93f4`。编译时嵌入，使用时重新验摘要，按 16 kHz mono 读取，每段后加一秒静音并循环至 4,800,000 个 f32 样本。这里只生成一份完整 PCM 并移交给实际 actor、VAD、stdio 和 ASR；不机械切分成多个结果。

使用实际 UI 的“开始听写”“完成听写”“取消”。完成按钮提交固定 300 秒样本，页面录音计时仍显示按按钮后的真实等待时间，所以它不证明墙钟五分钟录音、CoreAudio、重采样或 TCC 行为。测试提示明确说明这一点。启动准备关闭此独立应用的录音快捷键、整理和历史，使用仅本机处理及手动交付；验证不点击复制或授权按钮。

## 进程归属与读数

```sh
python3 scripts/diagnostics/macos-app-memory.py \
  'target/release/bundle/macos/SpeakMore Model Check.app' --seconds 300
```

先以普通桌面方式打开测试应用，此脚本只观察，不启动、控制或终止应用。输出进入新的 `.cache/app-memory-study/run-*`，保留全部 JSONL 采样及报告。它严格检查测试应用标识，并记录可执行文件、worker、采样器和源码摘要。

只沿父子 PID 关系会漏掉由 launchd 托管的 WebKit/XPC 服务。C 诊断按应用的 **resource coalition** 获取成员；精确主程序路径、PID 和进程启动时间共同锁定观察对象。拒绝与采样器或 launchd 共用的资源组，避免把调用宿主或系统组当成应用。

该诊断依赖未在本机公开 SDK 中声明的只读接口布局，仅用于开发检查，未链接进产品。定义依据为 Apple [XNU `proc_info_private.h`](https://github.com/apple-oss-distributions/xnu/blob/xnu-11417.140.69/bsd/sys/proc_info_private.h) 的 coalition 元数据和 [coalition 类型/计数](https://github.com/apple-oss-distributions/xnu/blob/xnu-11417.140.69/osfmk/mach/coalition.h)。只读取 usage 结构开头的 started/exited 两个计数，不推测完整结构的 footprint 偏移。[系统调用实现](https://github.com/apple-oss-distributions/xnu/blob/xnu-11417.140.69/bsd/kern/sys_coalition.c) 和 [libsyscall 包装](https://github.com/apple-oss-distributions/xnu/blob/xnu-11417.140.69/libsyscall/wrappers/coalition.c) 提供了该读取行为的依据。其他 OS 版本仍须重新验证；接口不可用时报告失败。

每次读数前后各枚举一次成员，逐项核对启动身份、父 PID、可执行文件和资源组。前后内核任务计数必须不变，且 started − exited 与枚举人数一致，才算完整样本。退出、新建、PID 复用、成员读取失败或数量不符均保留为不完整，不计入合计峰值。只枚举 PID/资源组元数据，路径和 RSS/footprint 仅输出目标资源组成员，不读取命令行、窗口内容、进程内存正文或其他应用数据。

合计是同一轮各进程**当前** RSS/physical footprint 之和，不相加独立的历史峰值。读取为近同时观测而非原子快照，短于采样间隔的峰值可能遗漏；RSS 可能重复计入共享驻留页。未把 WindowServer 等共享系统服务整体归入该应用。资源组完整只说明此轮计数与身份闭合，不保证 OS 将所有间接资源开销都归入它。

## 验收边界

保留主进程 300 MiB、整应用 2 GiB、连续 30 分钟并含三次物理五分钟录音、M1 8 GB/macOS 14、自然长语音质量等要求。采样工具和公开音频构建只能提供局部证据，不能把默认生产 SenseVoice 原图的既有超预算问题改写为已解决；候选下载发布、质量和正式安装验收仍须独立完成。

## 2026-09-16 实测

M4 / 32 GiB / macOS 15.6.1；release 测试应用，候选 `sensevoice-tiled256-v1`，CPU worker，自动识别语言。沿用前次经真实 UI 导入的候选安装和 revision 7，没有直接改写数据库选择。普通应用未启动。

连续完成三次固定 300 秒输入，界面分别出现 1、2、3 条结果，各显示 05:00。第四个任务在界面“正在转写”时取消，经历“正在取消”后重新可开始，结果与用量均仍为 3；随后再次完整转写成功，最终为 4 条结果、4 个 usage event。没有点击复制，历史为 0 行、服务和词典均为 0，SQLite v7 / integrity ok。操作通过实际界面和常规 IPC，数据库只读核对计数。结果侧栏显示原文；原生观察工具截断长文本字段，本轮不声称完成全文摘要一致性比较，也不把界面观察时间间隔当作 ASR 精确耗时。

[处理中截图](./screenshots/macos-public-audio-processing.jpg) 展示测试标识和取消入口；[三次处理后的首页上部](./screenshots/macos-public-audio-completed.jpg) 展示最新结果与 05:00。截图窗口高度只容纳最新结果，三条结果数量由对应完整辅助功能树及数据库计数交叉确认。

资源组完整样本最多 7 个成员：主程序、两个 WebContent、WebKit GPU、WebKit Networking、ThemeWidgetControlViewService 和一个 SenseVoice worker。后五个系统服务的父 PID 均为 1，证实仅统计应用子树会漏算它们。取消前后观察到不同的 worker 启动身份；每个完整样本最多一个 worker。退出应用后，所有已记录的主程序、worker 与 XPC 进程身份均确认不再存活。

| 指标                             | 实测                                                   |
| -------------------------------- | ------------------------------------------------------ |
| 全序列时间                       | 388.312 秒；不是 30 分钟或物理五分钟录音               |
| 采样                             | 3,805 次，3,804 完整；末次主程序退出，标为不完整并排除 |
| 合计 RSS 采样峰值                | 1,640,497,152 bytes / 1.528 GiB                        |
| 合计 physical footprint 采样峰值 | 1,414,977,632 bytes / 1.318 GiB                        |
| 主进程 RSS 采样峰值              | 106,463,232 bytes / 101.531 MiB                        |
| 读取窗口                         | 中位数 5.963 ms，最大 59.750 ms                        |
| 采样间隔                         | 目标 100 ms，最大 155.079 ms                           |

采样包含初始 worker 加载、四次完成的转写、取消/重载和结果侧栏；开始前的部分应用启动不在范围内。人工操作、系统负载和温度未受控，不由这些数值发布 P95 或最低配置承诺。RSS 和 footprint 的最高值发生在不同采样时刻，分别列出。

原始目录为 `.cache/app-memory-study/run-xtiqz41v`，UI 原生观察为 `.cache/app-memory-study/ui-20260916`。[结构化证据](./evidence/macos-gui-resources-2026-09-16.json) 记录构建、输入、设置、源码与观察摘要；[压缩采样](./evidence/macos-gui-resources-2026-09-16.samples.jsonl.gz) 保留全部尝试。采样器的无效参数、缺失应用、错误启动身份拒绝检查通过；两个新音频源检查、两个测试清单/准备检查、前端 46 项回归、TypeScript/Vite、原生 all-targets Clippy 与格式检查通过。

本轮发现中文结果侧栏关闭按钮仍读作 Close，已为结果详情和最近结果补齐本地化标签。修复后重新构建并打开最近结果，实际读作“关闭”，点击后焦点回到“查看全部最近结果”；[原生截图](./screenshots/macos-results-close-localized.jpg) 与[全部原生文本观察](./evidence/macos-gui-resources-2026-09-16.ui.jsonl.gz) 已保存。资源测量使用修复前构建，修复后的构建与观察单独记录，不能混用二者二进制摘要。该修复不改变转写、用量或采样方法。

最终普通应用和测试应用均重新构建，deep/strict 签名检查有效，但仍为 ad-hoc、未公证。普通包为 arm64 / macOS 14.0，不包含测试清单、固定音频采集入口或测试提示；生产签名目录逐字节等于发布基线。冻结 Input Check 摘要未变，测试应用已确认退出。
