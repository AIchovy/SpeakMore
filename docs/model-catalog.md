# 模型目录维护

应用只安装内置已签名目录中、当前原生引擎支持的文件包，可在线下载或从本机导入。首批发布平台为 macOS 14+ Apple Silicon；当前提供五项：Whisper small Q5_1、large-v3-turbo Q5_0、SenseVoice Small INT8、Parakeet TDT v3 INT8，以及 Qwen3-ASR 0.6B INT8。两种 Whisper 使用 Silero GGML VAD；其余三种使用 ONNX VAD v5 与独立 CPU worker。完整语料、低配机资源和商业许可/依赖 notices 仍须按发布 Gate 验收。

## 文件与信任

- `resources/model-sources.lock.json`：上游 revision、大小、SHA-256 与许可的来源记录。
- `resources/model-catalog.payload.json`：维护者编辑的完整文件包目录。
- `resources/model-catalog.signed.json`：对 payload 原始字节的 Ed25519 签名，不先重排 JSON 再验签。
- `resources/model-catalog.baseline.signed.json`：已发布目录的已签名连续性基线；构建与启动核对当前目录仍保留其中全部安装身份及摘要。
- `resources/model-trust.json`：应用内置的模型目录公钥。与未来许可、应用更新签名钥隔离。
- `.cache/signing/model-manifest.key`：本工程初始化时生成的私钥，目录 0700、文件 0600，`.cache` 已被忽略。只能由维护者保管；不能提交、上传、打印或打包。发布前须将其纳入有备份和权限控制的密钥管理流程，不能把缓存目录作为长期唯一备份。

维护流程：确认真实产物和许可，修改 payload，然后用已有私钥签名。工具在公钥已存在时拒绝使用另一把私钥覆盖信任根；轮换需要明确维护公钥与兼容策略。

```sh
cargo run -p speakmore-models --features signing --bin speakmore-model-catalog-sign -- \
  resources/model-catalog.payload.json \
  .cache/signing/model-manifest.key \
  resources/model-catalog.signed.json \
  resources/model-trust.json
```

签名工具仅维护者显式构建，默认产品构建不生成这个二进制。桌面 build.rs 校验签名、目录合同与源稿语义一致性；运行时再次验签。每个 bundle 固定引擎/profile、最低版本、平台、语言、能力与全部附件的 revision、大小、SHA-256、URL、许可。当前 URL 限定固定 Hugging Face revision；HTTPS 重定向仅允许 huggingface.co / hf.co 及其子域名，最多 4 次。

## 安装与使用

模型文件写入应用私有模型目录，staging 与最终目录在同一文件系统；单任务、顺序下载附件。总大小预检包含 64 MiB 余量。请求连接 10 秒、无新数据 30 秒截止；网络/5xx 最多重试两次，等待 1 秒、3 秒，每次从当前附件头开始；4xx、大小/哈希错误与磁盘失败不自动重试。已经验证完成的附件在本次任务中保留，整个任务失败/取消才清理 staging。

网络流和落盘文件都校验，fsync 后原子提交整个目录，再登记 SQLite。提交阶段不能接受取消；在此之前接受的取消保证不提交。数据库登记失败保留已验证文件，下次启动重新校验后登记。Installed 仅表示文件完整且已登记，模型 Loading/Ready 由 worker 监督器确认。

加载前取得 `ModelLease` 并重新校验全部文件；在候选模型加载、当前模型驻留及子进程完全退出之前保持租约。ModelManager 拒绝删除有租约的模型。安装本身不会选择模型。设置中的使用/停用/重载与会话资源调度已交给唯一 actor：校验与 worker Ready 后才事务提交选择，保留其他有效设置修改；失败恢复先前模型。当前模型先停用、等待进程回收与租约释放，再允许删除。录音期间不能切换模型/设备、开始模型下载或导入、移除文件。原生下载与跨家族选择已实测；本机导入的取消和错误恢复已实测，完整导入及推理由临时目录中的真实模型集成测试验证。物理麦克风与五分钟整应用验收仍待完成。

启动会清理自有 UUID staging 目录、重新校验当前目录中的产物、恢复已提交而未登记的 bundle，并清除缺失或损坏的登记。不会递归删除未知文件树，也不跟随符号链接。不同版本分别登记、校验和取得租约，某个新版本缺失或损坏不会清除旧版本的登记与选择。

## 模型版本升级与回退

同一逻辑 `model_id` 的新产物作为独立目录条目追加，指定可读的 `version` 标签（最多 40 UTF-8 字节，不含控制字符或首尾空白）。原有三项条目不增加该字段，原始序列化摘要、安装身份和文件位置保持不变。带标签条目的原生安装 ID 为 `model_id--完整 bundle 摘要`；摘要包含标签及整条模型记录，相同家族/标签不能对应两项产物，所有安装 ID 唯一且不超过 120 字节。目录继续最多 32 项，包含保留版本，超限阻止发布而不自动丢弃旧版本。

`ModelRow.modelId`、下载/导入/选择/删除命令、SQLite `models.model_id`、所选模型和租约使用安装 ID；`familyModelId` 表示逻辑模型，`version` 用于界面展示。磁盘保持 `bundles/{逻辑模型 ID}/{bundle 摘要}`，旧文件不移动。supervisor 分别持有安装 ID 与逻辑模型 ID：前者用于生命周期事件、设置和结果记录，后者沿用 stdio v2 的固定引擎合同；两者与 bundle 摘要绑定，拒绝任意别名。此变更不修改 worker 协议或数据库 v7。

升级应用后先校验旧版本并保留原选择，新版本显示为可下载的独立项。安装新版本不切换当前模型；用户明确选择后，旧 worker 先卸载，新版本实际 Ready 且 SQLite 选择事务成功才成为当前模型。加载失败或选择提交失败恢复旧版本，录音中拒绝切换；随后的结果记录精确安装身份。用户可选择任一已安装的保留版本进行回退。移除仅删除该安装身份的登记和目录；正在使用的版本必须先停用，有租约的文件不能移除，其他版本、历史和用量不受影响。不自动清理旧版本。

发布维护不得原位修改、删去已有条目或通过复用旧逻辑 ID 替换文件。`TrustedCatalog::preserves` 在构建和启动时校验连续性；`baseline.signed.json` 保留上一已发布目录的原始已签名字节，每次发布后将其推进到该次正式发布目录，下一次修改源稿前完成此步骤并保留发布归档。不要把未发布候选当成基线来绕过检查。当前基线与原目录相同，本轮没有新增真实模型版本、改签生产目录或读取私钥。需要撤回旧版本或超过容量时，先设计明确的数据迁移/撤回策略；不能直接让旧条目消失。

验证覆盖合成目录升级、旧路径摘要不变、错误新文件、SQLite 重开后旧选择与设置修订保留、同家族成功切换/回退、加载失败、选择提交失败、录音互斥、一次记账、每版本租约和单独删除。真实公开模型另以仅测试信任的版本标签完成离线导入、SQLite 重开和实际 CPU worker 转写；这验证版本身份链路，不代表发布了新的权重或通过所有安装升级/断电/最低机型验收。

## 从本机导入

在“设置 → 本地模型”中选择未安装模型的“从本机导入”，按弹窗列出的原文件名准备全部解压后文件，在原生文件选择器中一次选中。Whisper 需要模型与 VAD 两个文件，SenseVoice 需要模型、tokens 与 VAD 三个文件。当前导入只接受目录列明的固定版本与哈希，不加载任意用户模型，也不安装压缩包。

`import_model` 只接受模型 ID，在 Rust 内部打开 NSOpenPanel。选择器与应用规则的选择器共用占用锁；取消返回空结果。读取路径、校验本地普通文件并持有只读文件描述符均在原生端完成，路径不交给 WebView。文件数量、名称、大小必须匹配目录；拒绝目录、符号链接、FIFO、缺失和额外文件。准备完成后，由会话 actor 再检查空闲条件，再启动安装任务。

导入不会联网，也不移动、改写或删除源文件。复制按 128 KiB 块进行，检查取消、大小与 SHA-256；之后复用下载的落盘复核、fsync、原子提交与 SQLite 登记流程。下载、导入和删除共用单任务占用，快照的 `jobKind` 区分操作；既有 `cancel_model_download(jobId)` 同时取消下载和导入。提交开始后不再接受取消，已接受的取消必须完成 staging 清理后才释放占用。外部磁盘的系统读取若卡住，任务保持占用直到系统调用返回，不能用另开无限任务伪装取消完成。

校验通过后模型显示“已安装”，仍需用户选择“使用模型”。已安装项不提供覆盖导入；导入错误、取消或选择器关闭均不会切换当前模型。

## 验证

```sh
npm run test:models
# 需要已构建的 worker 及 .cache/asr-fixtures/jfk.wav，只使用公开语音样本。
npm run test:models:download
# 需要 .cache/asr-fixtures 和 .cache/sensevoice/model 中的已校验产物及两个 worker；不联网。
npm run test:models:import
# 同样离线；以仅测试信任的版本标签检查两种实际 worker 的版本身份链路。
cargo test --offline -p speakmore-desktop --features session-fixtures,target-audit --lib versioned_offline_import_sqlite_reopen_and_real_worker -- --ignored --nocapture
```

`test:models` 用仅在单元测试中编译的回环 HTTP 服务与临时文件验证取消、重试、损坏响应、原子提交、数据库登记失败恢复、租约与符号链接边界；没有产品级 HTTP 放宽开关。导入测试还覆盖打开后源路径替换、内容修改、截断/增长、复制中取消及重试，并断言无网络请求。

`test:models:download` 实际下载约 191 MB 文件包到临时目录，调用桌面 SQLite 适配器，重启复核并用真实 CPU worker 转写公开样本，最后卸载并清理。`test:models:import` 对 Whisper small 和 SenseVoice Small 分别完成离线导入、SQLite 重开与文件复核、真实 CPU worker 转写和租约保护，使用临时模型目录和数据库，不改写用户模型或缓存中的源文件。默认检查不重复这些大文件专项测试或外部下载。

## SenseVoice 产物与能力

已签名文件包总计 241,862,836 bytes：模型 239,233,841 bytes、tokens 315,894 bytes、VAD 2,313,101 bytes。模型与 tokens 固定 Hugging Face revision `2365baeacb507f821a0c8120fcee3d484dba7a07`；VAD v5 固定 `fba88cd2e921609e7675c3aaf51e0b9b295da4bc`。全部 SHA-256 / URL 以签名 payload 为准。`resources/sensevoice-fixtures.lock.json` 是开发与公开样本来源记录，不能作为生产未签名目录绕过验证。

模型与 tokens 使用 [FunASR Model Open Source License Agreement](https://raw.githubusercontent.com/modelscope/FunASR/main/MODEL_LICENSE)（当前核对 1.1），VAD 为 MIT；sherpa-onnx 的 Apache-2.0 只覆盖对应代码。发布前还需完整原生依赖 notices 和商业分发核对，不能把此技术接入等同于许可放行。

`sensevoice-v1` 固定 CPU 最多 4 线程、ITN 和整段推理；支持 auto 检测中文、粤语、英语、日语、韩语，全局显式语言设置仍为 auto/zh/en。不支持个人词典 prompt，使用数为 0、省略数为全部候选数，结果标记需检查。设置页显示这项能力差异和模型许可。

协议 v2 在握手与 Ready 中核对引擎身份、profile 和文件包摘要。跨家族先回收旧进程，再从固定包路径启动对应 worker；错误引擎握手、模型校验或加载失败时恢复原家族。原有 Whisper bundle 定义与摘要未改变，新增模型不替换现有已安装版本。

### 未发布的五分钟资源优化候选

代码增加 `sensevoice-tiled256-v1`，对应 [完整上下文注意力分块实验](./sensevoice-tiled-attention.md) 的 256 行图。它沿用逻辑 ID `sensevoice-small-int8`，必须带独立 `version`，由完整 bundle 摘要生成不同的安装身份。目录验证、协议及原生加载同时要求主文件为 239,537,160 bytes、SHA-256 `46346d31c9314b4873aa16882f20955774cda4048a849c80db2a27117569a3d8`；其他图不能借用该 profile。转写请求必须匹配实际加载的 profile，不能在旧图与新图之间仅改请求标签。

它保留 CPU 最多 4 线程、原 VAD/ITN/语言/词典和 300 秒限制。受信生产目录仍只有原三个条目，普通应用设置页暂不提供该候选下载或导入。独立开发应用使用固定测试信任进行 [原生界面验证](./model-ui-check.md)，不能将其占位下载地址或公用测试签名者用于发布。开发测试使用独立生成报告与临时版本身份，不新增运行时路径或未签名目录入口。正式发布前还需整应用资源、最低机型、自然长语音、依赖许可和版本升级验收，并设置实际发布的最低 worker 版本；不能把测试目录中的占位 URL 当作发布源。

## 新增模型（2026-10-02）

- Parakeet TDT v3 INT8：英语及另外 24 种欧洲语言，自动识别；不支持中文。主 encoder、decoder、joiner、tokens 与 VAD 共五个文件，约 672.8 MB。
- Qwen3-ASR 0.6B INT8：30 种语言及中文方言，自动识别。encoder、conv_frontend、decoder、三项 tokenizer 文件与 VAD 共七个文件，约 989.3 MB。这是语音识别模型，与用于整理和翻译的 Qwen3 4B 文字模型分别下载。
- Zipformer English 20M：仅英语的轻量流式字幕模型，INT8 encoder/joiner 与 FP32 decoder，约 45.2 MB。在会议与字幕模型目录按需下载，字幕页单独选择，不替换现有中英模型。

两款新听写模型沿用已固定的 sherpa-onnx 1.13.8 / ONNX Runtime 1.28.2，无新增 Python 环境。`speakmore-sensevoice-worker` 的固定可执行路径保留，新模型由独立 profile 分派。非主图文件通过协议的可选 `auxiliary` 字段传入，必须匹配 profile 的完整角色/文件名、同一 bundle 目录和各自 SHA-256；原模型的空附件不序列化，原条目及安装摘要保持不变。发布连续性仍对已签名 baseline 校验。

新模型自动识别语言；不应用 SenseVoice 的显式语言与检测语言合同，也不伪造词级时间戳。目前个人词典提示尚未接入这两款模型。用户可继续用已配置 AI 服务进行后续整理。Parakeet 许可为 CC BY 4.0，Qwen3-ASR 与 English Zipformer 来源标记为 Apache 2.0；出处与转换说明随应用附在 `workers/ASR-models-NOTICE.txt`。

五分钟听写限额不变。新模型每次原生解码控制在约 20 秒，优先在最后三秒中的安静处切分，逐段连接原文；每个原音频样本处理一次，跨段间不提供上下文提示。保留 VAD、取消与子进程监督。词典使用数明确为零。

验收范围：目录签名/原条目连续性、协议拒绝缺失/重复/越界附件、分段完整性、前端构建与中英文说明、每款新模型一段上游公开录音的实际加载和转写。未使用用户麦克风或修改用户数据；真实口音、噪声、五分钟长录音与最低配置 Mac 的效果仍需人工验收。公开样本与权重位于项目 `.cache/additional-asr/`，不会进入安装包。

本次公开音频检查已通过：Parakeet v3 使用上游英语样本（24 kHz，经应用同款 DSP 转为 16 kHz），Qwen3-ASR 使用上游日语样本，两者经过真实监督器和 release worker；English 20M 通过安装包中的流式动态库逐块解码英语样本。短样本转写通过仅证明接入可运行，不代表完整准确率/性能评测。项目本机上两款非流式样本的纯转写各约 1.45 秒；这不是不同模型之间的公平速度对比。
