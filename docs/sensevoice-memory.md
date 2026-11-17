# SenseVoice 五分钟推理内存调查

2026-09-16。结论：**尚未通过 small 整应用峰值 ≤2 GiB 的发布门槛**。同一个默认 native recognizer 连续处理三次 300 秒公开样本，单进程最大 RSS 达 2.185 GiB；仅观察第一次推理会漏掉这个问题。本次没有修改产品推理配置、模型、录音时长、分段方式或取消机制。所有候选仅在独立开发诊断程序中运行。

## 测量范围

使用此前记录的本机 Apple M4 / 32 GB / macOS 15.6.1，CPU 4 线程；固定 sherpa-onnx 1.13.8、ONNX Runtime 1.28.2 与现有 SenseVoice INT8 文件。编译器为本机 `/usr/bin/clang++`，诊断程序使用 C++17 / `-O2`。运行前核对仓库中静态库、C API header、模型、tokens、VAD 和公开英语 fixture 的文件大小与 SHA-256；不重新下载模型。

输入为已锁定 `en.wav`，每段追加 1 秒静音后循环并截取到 4,800,000 个 16 kHz 样本，恰好 300 秒。每次完整输入只调用一次原生 decode。ITN 开启，显式英语，greedy search。每个配置在全新进程中加载 recognizer；`--repeats 3` 复用同一个 recognizer，逐次释放 stream/result。实验不运行产品 VAD，不打开麦克风，不启动主程序/WebView，不访问历史、凭据、生产私钥或系统权限。

通过 macOS SDK 的 `proc_pid_rusage` 和 `getrusage` 记录当前 RSS、进程生命周期最大 RSS、当前/峰值 physical footprint。**两种内存指标分别报告，不互相替代；节点分配统计也不等于进程峰值。** 所有字节值和每次时长保留在[原始测量记录](./evidence/sensevoice-memory-2026-09-16.json)。

## 结果

表中“连续次数”是一个进程内的调用次数，最大值覆盖该进程整个生命周期。GiB 按 2³⁰ bytes 换算。

| 配置                                                                   | 连续次数 | 最大 RSS / GiB | 峰值 footprint / GiB | 各次 decode / 秒         |
| ---------------------------------------------------------------------- | -------- | -------------- | -------------------- | ------------------------ |
| 默认 CPU                                                               | 1        | 1.767          | 1.720                | 29.686                   |
| 关闭 CPU arena                                                         | 1        | 2.321          | 1.368                | 39.604                   |
| 同时关闭 arena / memory pattern                                        | 1        | 2.274          | 1.370                | 39.371                   |
| 默认 CPU                                                               | 3        | 2.185          | 2.201                | 33.811 / 32.333 / 32.635 |
| 仅关闭 memory pattern                                                  | 3        | 1.921          | 1.744                | 29.835 / 32.356 / 34.907 |
| 按请求大小扩展共享 arena，关闭 pattern                                 | 3        | 2.386          | 2.465                | 32.899 / 30.561 / 30.256 |
| 上项并将未使用块拆分阈值设为 1 MiB                                     | 1        | 2.068          | 2.434                | 31.338                   |
| 共享 arena 使用倍增、扩容步长阈值 32 MiB、拆分阈值 1 MiB，关闭 pattern | 1        | 1.810          | 1.680                | 35.022                   |
| 关闭 pattern 并收集节点统计                                            | 1        | 1.784          | 1.699                | 31.181                   |

这些对照序列顺序运行，没有同时运行两个解码实验。没有控制其他系统负载或内存压缩，不能把细小数值差异当作稳定收益，也不能计算产品 P95。扩容步长阈值不是总内存上限，大请求仍会直接分配。所有表中 15 次公开样本转写得到相同的 2,570-byte 文本 SHA-256，且包含 37 次预期的 `tribal`：`67658f46ee7ca478852d1770349001892c8665696429fd7931b64698940657cb`。这只能证明本样本结果一致，不能代替自然长语音、混合语言、首尾覆盖或精度验收。

补齐报告完成标记、文件校验和 CSV 摘要后，以最终脚本另行完成 1 次节点统计模式验证：decode 32.577 秒、最大 RSS 1,870,938,112 bytes、峰值 footprint 1,849,214,872 bytes；3,412 个节点、上述最大分配及文本摘要一致。合计实际完成 16 次完整 300 秒输入的原生 decode。文件大小/摘要错误和符号链接拒绝分支另行验证；这次工具验证没有扩大产品性能结论。

默认配置的第 2 次调用已经使单进程最大 RSS 超过整应用预算。仅关闭 pattern 在该对照中降低了增长，但单进程仍达到 1.921 GiB，未给主程序、WebKit、VAD 和传输缓冲留出已验证的余量。关闭 arena 则出现 footprint 下降、RSS 上升且耗时增加的情况。按请求大小扩展的候选未改善占用；限制扩容步长的单次结果也不足以证明解决连续使用问题。**本轮不将这些配置接入产品，不降低原内存预算。**

## 分配来源

ORT 节点统计共记录 3,412 个节点。该 300 秒输入下：

- 注意力 `Where` 节点的 dynamic output 为 400,640,256 bytes，临时分配为 400,660,272 bytes；140 个 `Where` 节点报告了这一数量级的临时分配。
- CTC `MatMul_quant` 的 dynamic output 为 501,500,928 bytes，是该记录中最大的单节点 dynamic output。

这些记录将后续工作指向长序列注意力与 CTC 大张量，以及分配复用策略。不同节点的生命周期会重叠或复用，**不得将这些行相加作为峰值**；仅凭此报告也不能确定哪一项改动能够满足预算。原 CSV 和摘要随测量记录保留。

后续需要评估实际算子/图层面的优化；如改变模型图、权重或分段策略，必须使用独立受信版本和对应推理 profile，重新核对自然语料精度、边界、取消及完整 300 秒覆盖。单独调节内存分配参数也须完成连续使用、混合长度、多语言及真实 worker 的回归，不能只根据这个重复英语样本替换默认值。

后续已完成首轮 [完整上下文注意力分块实验](./sensevoice-tiled-attention.md)：保持完整键/值上下文与权重，将 70 层注意力的查询行分块。128 / 256 行两种候选各通过 13 对多语言与长短音频的原生 JSON 结果字段对照。后续采用 256 行作为验证候选：连续三次 300 秒的单进程最大 RSS 约 1.097 GiB，多语言序列约 1.235 GiB；仍须完整 worker 和整应用验收。候选尚未进入产品或签名下载目录，不改变本页关于现行产品未通过内存 Gate 的结论。

## 重现

先按 [README](../README.md#sensevoice-原生构建与验证) 准备已锁定的模型、静态库与公开 fixtures。通常的实验完全离线：

```sh
python3 scripts/diagnostics/sensevoice-memory.py
python3 scripts/diagnostics/sensevoice-memory.py --modes default no_pattern --repeats 3
python3 scripts/diagnostics/sensevoice-memory.py --modes node_stats
```

可选共享 arena 实验还需要与链接库同版本的官方 ORT C API header；以下显式命令仅下载三个固定 header 及许可证，验证大小和 SHA-256 后写入项目缓存。默认实验不会自行联网：

```sh
python3 scripts/diagnostics/sensevoice-memory.py --prepare-ort-headers
python3 scripts/diagnostics/sensevoice-memory.py --modes exact_arena --repeats 3
python3 scripts/diagnostics/sensevoice-memory.py --modes compact_arena growth_arena
```

输出在新建的 `.cache/sensevoice/memory-study/run-*` 私有目录中：数值 `report.json`、公开样本文本与 native stderr，节点模式另含 CSV。最终版本的报告只有全部调用成功且文本一致后才写 `completed: true`；历史测量保留当时的原始字段和成功退出记录。节点统计单独复核也不是新的性能基线。不要把该工具改接用户音频；它的文件校验与明文诊断输出都只针对公开 fixture。

本工具属于开发诊断，不加入应用构建、不进入最终安装包，不要求最终用户安装 Python。没有修改 `crates/sensevoice` 或 worker 的产品代码；既有取消/回收行为本轮没有重新验收。最终资源门槛仍须测量真实 worker、主进程与归属明确的 WebKit 进程在同一时间轴上的占用，并完成连续 30 分钟且含 3 次完整录音的测试和 M1 / 8 GB / macOS 14 验收。

## 原生接口依据

固定版本 sherpa-onnx 的 [session.cc](https://github.com/k2-fsa/sherpa-onnx/blob/v1.13.8/sherpa-onnx/csrc/session.cc) 提供 provider 配置文件中的 arena/pattern 开关与 `SessionConfig.*` 转发。分配策略采用 ORT 官方 [C API](https://onnxruntime.ai/docs/get-started/with-c.html) 的共享 allocator 配置；具体字段和节点统计设置对照 [1.28.2 session options](https://github.com/microsoft/onnxruntime/blob/v1.28.2/include/onnxruntime/core/session/onnxruntime_session_options_config_keys.h)。这些是实验机制的来源，内存收益只以本地测量判断。
