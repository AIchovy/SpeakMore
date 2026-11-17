# 公开语料识别质量诊断

本检查使用公开开发/验证集，帮助定位中文、英文和中英混合识别的问题。它不替代设计 §16 要求的自有、经同意、按说话者分离的最终验收集，也不用于修改验收门槛。结果中的“完成转写”只说明进程返回了结果，不代表文本准确或可自动输入。

## 2026-09-16 首轮结果

两个引擎分别完成全部 300 次尝试，诊断程序运行 743.97 秒，正常退出并回收 worker。所有失败保留在精度分母。以下是本批语料上的严格文本结果，不是官方数据集排行榜分数，也不是最终发布验收：

| 引擎                  | 语言 / 指标  | 错误 / 参考 token | 错误率 | 结果 / 失败 | 完成样本耗时 P95 | 完成样本 RTF P95 |
| --------------------- | ------------ | ----------------- | ------ | ----------- | ---------------- | ---------------- |
| SenseVoice Small INT8 | 中文 CER     | 310 / 3,529       | 8.78%  | 100 / 0     | 680 ms           | 0.0521           |
| SenseVoice Small INT8 | 英文 WER     | 272 / 2,146       | 12.67% | 100 / 0     | 686 ms           | 0.0507           |
| SenseVoice Small INT8 | 中英混合 MER | 269 / 1,527       | 17.62% | 99 / 1      | 472 ms           | 0.0746           |
| Whisper small Q5_1    | 中文 CER     | 1,082 / 3,529     | 30.66% | 100 / 0     | 2,512 ms         | 0.3645           |
| Whisper small Q5_1    | 英文 WER     | 210 / 2,146       | 9.79%  | 100 / 0     | 2,467 ms         | 0.3537           |
| Whisper small Q5_1    | 中英混合 MER | 566 / 1,527       | 37.07% | 100 / 0     | 2,171 ms         | 1.2432           |

P95 混合该语言实际采到的不同长度，不能与设计中某个固定时长的端到端目标直接比较；各时长桶及样本数保存在 [结构化证据](./evidence/asr-public-quality-2026-09-16.json) 中。模型加载时间独立记录，没有混入上表。

本次 SenseVoice 的中文错误率较低，Whisper small 的英文错误率较低；两个引擎的混合错误率均高于设计的 15% 目标。不能据此选择一个对所有语言都更好的默认模型。设置页将 Whisper small 的无条件“首次推荐”改为可核实的“体积较小”，没有自动更换用户的模型或解码配置。

唯一失败为 SenseVoice 对 `ascend/validation/00903` 返回 NoSpeech：1.82 秒语气词片段，原始参考包含两种文字。它仍按正样本漏识别计入，未因内容较少而剔除。Whisper 的混合结果有 3 段检测语言落到 ps/pl/ko，需继续复核短语音自动判语言。抽查中文高误差段同时出现繁简差异、人名/地名误识别、遗漏括号内英文和数字形式差异；尚未逐段人工标注占比，不能把 30.66% 全部归为繁简问题或全部解释为语义错误。

SenseVoice 的 299 个完成结果均带 NeedsReview；Whisper small 只有 19 段带该标记（中文 1、混合 18），英文为 0。该标记不是已校准的准确率保证，本轮不调整阈值或放开自动输入。turbo 对照见下节；后续继续评估已知语言提示与中文输出形式，建立独立自然混合/短句/关键片段和负样本集，再确定默认模型及质量策略。

## 同日 turbo 补充诊断

使用完全相同的冻结 runner、300 段 manifest、worker、CPU/auto、空词典和计分器，另在 `run-v3-turbo` 保存一次完整运行；未覆盖首轮的 600 条记录。Whisper large-v3-turbo Q5_0 的全部 300 次尝试及终态回收完成，程序总耗时 2,607.74 秒，模型加载为 1,313 ms。结果与逐段非正文计数见 [turbo 结构化证据](./evidence/asr-turbo-quality-2026-09-16.json)。

| 语言 / 指标 | 错误 / 参考 token | 错误率 | 完成 / 失败 | 完成样本耗时 P95 | 完成样本 RTF P95 |
| --- | --- | --- | --- | --- | --- |
| 中文 CER | 314 / 3,529 | 8.90% | 100 / 0 | 8,448 ms | 1.5289 |
| 英文 WER | 165 / 2,146 | 7.69% | 100 / 0 | 9,687 ms | 1.6215 |
| 中英混合 MER | 456 / 1,527 | 29.86% | 99 / 1 | 11,299 ms | 5.9067 |

运行期间进行了前端测试、增量 Rust 检查与测试、独立 UI 测试包构建，存在共享 CPU 负载。这些耗时不是隔离性能对照，不能据此宣称固定速度差异或通过性能 Gate。唯一失败为约 2.94 秒的 `ascend/validation/00067`，15,462 ms 后 AsrTimeout；仍按空预测计入分母。尚未在独立负载下复核该失败，不能把超时直接归因于模型或并发编译，也不能删除它再报告更低错误率。

本批 turbo 的英文严格错误率低于另两模型，中文与 SenseVoice 接近；混合错误率仍高于 SenseVoice，且高于设计目标。99 个完成的混合结果检测为 zh 60、en 37、ru 1、id 1；其中 7 段 NeedsReview，中文和英文均未标记。没有将自动判语言或未标记结果视为正确性保证，没有更改默认模型、解码参数、阈值或自动输入资格。后续需在隔离负载下复核短句超时，并测试 Metal、已知语言提示和独立自然语料。

## 语料与选择

固定来源见 [下载锁文件](../resources/asr-quality-sources.lock.json)。只下载以下三个开发分区，不访问测试分区，不执行数据集远程代码：

| 语言     | 来源                              | 固定版本                                   | 候选池与抽取                                                  |
| -------- | --------------------------------- | ------------------------------------------ | ------------------------------------------------------------- |
| 普通话   | Google FLEURS / cmn_hans_cn / dev | `70bb2e84b976b7e960aa89f1c648e09c59f894dd` | 409 段、150 个不同提示文本 ID，取 100 个 ID 各一段            |
| 英语     | Google FLEURS / en_us / dev       | 同上                                       | 394 段、150 个不同提示文本 ID，取 100 个 ID 各一段            |
| 中英混合 | CAiRE ASCEND / validation         | `737e9800ae31be9932ba8464c80366559bd28424` | 1,130 段中 213 段标注 mixed 且同时含汉字和拉丁字母，取 100 段 |

推理前，以固定种子 `speakmore-public-development-v1` 加数据集、版本及源 ID 的 SHA-256 排序选择。FLEURS 同一提示文本只保留排序最先的录音。没有按模型预测挑选样本。FLEURS 使用 TSV 第三列原始参考文本，不使用其预先归一化的另一列；ASCEND 使用发布者 transcription 字段。

总计 300 段、2,485.9609375 秒，单段 0.84–31.74 秒。混合子集只有 3 位可核验说话者（发布者 ID 7、12、20）；不满足十位说话者要求。所有音频原采样率为 16 kHz 单声道，统一写为 PCM16 WAV；不剪静音、不循环、不拼接、不改变语速。记录原音频及转换后 WAV 的 SHA-256、样本数与超幅采样数，本次源文件无超幅采样。下载按源文件大小及 SHA-256 校验，小型 TSV 按发布者 Git blob SHA-1 校验；整个最终 manifest 另锁定 SHA-256。未通过完整校验的文件不会参与评测。

FLEURS 是朗读语音，不能代表所有噪声及自然口述场景；当前 TSV 不含说话者身份，不能通过性别字段推断说话者数量。ASCEND 是自然中英对话，但本次子集仍不是产品使用者的独立验收录音。两者可能与模型训练数据重叠，未证明无污染。[FLEURS 数据卡](https://huggingface.co/datasets/google/fleurs/blob/main/README.md)、[ASCEND 数据卡](https://huggingface.co/datasets/CAiRE/ASCEND/blob/main/README.md)。

## 执行路径与计分

显式运行的 [原生诊断程序](../crates/asr-worker/tests/quality_corpus.rs) 使用应用已打包的 release worker、生产模型目录中的锁定权重、真实 Supervisor、stdio 协议和 VAD。父级为 debug 诊断程序，前端/桌面 actor/采集/自动输入不在本次路径中。模型依次加载，不并发推理；仅 CPU、语言 auto、空词典、无文字整理。普通应用、用户历史和系统权限不参与。

Whisper 使用 `whisper-v1`；SenseVoice 使用原始 `sensevoice-v1`，不是尚未发布的分块候选。线程配置来自实际 worker：Whisper 为可用并行度与 8 的较小值，SenseVoice 为可用并行度与 4 的较小值；诊断没有额外上报运行时最终线程数，不把上限写成观测值。worker、runner、目录、模型、VAD 与语料摘要写入证据。模型加载耗时单列。

[计分器](../scripts/diagnostics/score-quality-corpus.py) 的 `speakmore-strict-text-v1` 在推理前定义：

- NFKC 与大小写归一化，按代码中的明确列表忽略标点。数字不展开为读音，否定词不删除，繁简不互转，不做词干化。数值小数点、正负号、百分号、词内句点/撇号以及 `@ / _ -` 保留。比如 `3.5` 与 `35`、`-5%` 与 `5` 仍然不同。问号等指定标点会删除，因此本指标不等同于 URL 或代码的逐字完全正确率。
- 中文 CER 按非空白字符；英语 WER 按归一化后的空白词界；混合 MER 按单个汉字与连续非汉字词计分。带连字符的英文、缩写、数字转写形式及繁简差异可能产生严格文本错误，因此不能将该错误率直接解释为语义错误率。
- 使用 Levenshtein 编辑距离，逐语言累加替换、删除、插入和参考 token 数后求比率；不平均每段百分比。失败或取消以空预测计入全部参考 token 的删除，不剔除难例。
- 同时记录整段完全匹配数、失败类型和质量标记。没有人工标注金额、否定、链接等关键片段，关键片段完全正确率保持未测。
- 耗时从 Supervisor 提交前到终态，包含传输、VAD 和解码，不包含加载与 WAV 读取，也不是目标应用上屏延迟。完成样本报告 nearest-rank P50/P95、最大值、RTF 和分时长桶样本量；失败单列，另保存全部尝试时长。少于 30 个观测的桶明确不满足设计要求。
- 缺少完成标记、样本遗漏、重复、输入摘要变化或未知结果均拒绝出报告。预测与参考全文只留在项目忽略的缓存；仓库证据记录样本 ID、摘要及误差计数。

`release_gate_passed` 固定为 false，因为该工具只负责公开数据诊断；不能用所有结果 ManualOnly 来声称负样本自动输入验收通过。

## 运行环境与限制

2026-09-16 使用 Apple M4 / Mac16,1 / 32 GiB、macOS 15.6.1 (24G90)，接电、低电量模式关闭。系统未报告热/性能告警；没有读取实际温度或控制室温，不构成热稳态测试。没有外接录音设备，本次只读公开 WAV。

Metal、M1/8 GB/macOS 14、五分钟自然录音、至少十位可核验说话者、负样本、关键片段、词典收益、实际输入与真实麦克风/端到端延迟均未由本次检查覆盖。turbo 的 CPU 结果有上述并发负载限制。公开集上的结果不能直接放行设计精度或性能 Gate。

## 复现

在项目根目录准备独立 Python 3.12 环境，按 [固定依赖](../scripts/diagnostics/quality-requirements.txt) 安装 pyarrow、soundfile 及辅助依赖。数据和环境只进入 `.cache/asr-quality`，不进入应用包。

1. 运行 `scripts/diagnostics/download-quality-corpus.py`，校验下载锁。
2. 运行 `scripts/diagnostics/prepare-quality-corpus.py`，创建全新的 `corpus-v1/manifest.json`；已存在时拒绝覆盖。
3. 准备模型与 `resources/workers` 中的打包 worker，运行 `cargo test -p speakmore-asr-worker --features whisper,sensevoice --test quality_corpus --no-run`。为本次运行复制固定 runner 到新的缓存目录。
4. 设置 `SPEAKMORE_QUALITY_MANIFEST` 为该 manifest 的绝对路径，`SPEAKMORE_QUALITY_OUTPUT` 为尚不存在的 JSONL 绝对路径；运行 runner 的 `--exact transcribe_pinned_public_development_corpus --ignored --nocapture`。两条路径必须在 `.cache/asr-quality` 中。
5. 以 manifest、结果 JSONL、尚不存在的报告路径作为三个参数运行 `score-quality-corpus.py`。报告必须有每个模型全部 300 个结果。
6. 运行 `python -m unittest discover -s scripts/diagnostics -p test_quality_score.py` 验证计分、失败计入、输入完整性和分位数规则。

初次 runner 把空词典修订号写成 0，被生产协议在提交第一段之前拒绝。改为合法修订号 1 后重新冻结程序并从全部 300 段重新开始；旧失败记录保留，未当作模型识别失败或成功样本。

turbo 补充运行另设 `SPEAKMORE_QUALITY_MODELS=whisper-large-v3-turbo-q5_0`，沿用相同 runner 副本及 manifest，使用新的输出目录。等待原执行句柄终态和结果完成标记后才计分；等待超时不代表进程已终止，不重复启动或覆盖既有输出。

## 数据归属

FLEURS：Alexis Conneau 等，_FLEURS: Few-shot Learning Evaluation of Universal Representations of Speech_，2022；发布者标注 CC-BY-4.0。[论文](https://arxiv.org/abs/2205.12446)、[数据集](https://huggingface.co/datasets/google/fleurs)。

ASCEND：Holy Lovenia 等，_ASCEND: A Spontaneous Chinese-English Dataset for Code-switching in Multi-turn Conversation_，LREC 2022；数据为 CC-BY-SA-4.0，不能与项目代码的 MIT 许可混淆。[论文](https://aclanthology.org/2022.lrec-1.788/)、[数据集](https://huggingface.co/datasets/CAiRE/ASCEND)。本地转换及抽样仅用于诊断；音频和参考文本没有作为 SpeakMore 产品资产分发。
