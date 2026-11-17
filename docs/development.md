# 开发指南

本地语音听写桌面应用。界面使用 React、shadcn/ui、React Icons 和 Motion，原生端使用 Tauri / Rust。

**macOS 优先，Windows 后续。** 首发基线 macOS 14+ Apple Silicon；设置中管理多个本地 ASR/STT 模型，当前实现 Whisper 与 SenseVoice 两家族。用户直接选择、下载和使用，无需自行安装 Python 或模型服务。

**开发中，尚不可作为商业版本发布。** 当前桌面会话 actor 已连接音频采集/DSP、受信模型租约、独立 Whisper / SenseVoice worker、结果缓存与本地存储。设置支持模型下载、取消、校验、安装、使用、切换、停用、重载及移除；概览支持点击听写、结束、取消，并展示最近 5 条结果；完整内存面板保留最多 10 条 / 15 分钟，支持原文对照、交付状态和显式复制。历史页提供搜索分页、批量选择/删除及通过 macOS 保存窗口导出 TXT/CSV。真实公开音频通过 actor 和实际模型完成转写；原生 WebView 已实际完成 SenseVoice 下载、选择与重启加载；默认 macOS 全局快捷键、非激活录音条和菜单栏控制已接入，物理按键/麦克风和外部焦点仍待专项验收。系统/显示器休眠、用户会话切换及关机通知已接入取消与 worker 回收，恢复时重新校验模型；原生通知适配和故障注入回归已通过，合盖/锁屏/唤醒真机验收仍待完成。自定义快捷键已接入持久保存、注册冲突恢复与编辑时的录音隔离。长音频资源优化、锁屏检测、自动输入、BYOK 与正式签名更新继续实施。持续证据见 [实施记录](./implementation-status.md)。

## 开发

需要 Node.js 22.12+ 或 24、Rust 1.91+（当前验证为 1.91.1）、CMake、libclang 与原生平台工具链。macOS 需要 Xcode；Windows 需要 Visual Studio C++ 工具链与 WebView2。具体依赖由 package-lock.json 和 Cargo.lock 锁定。

```sh
npm ci
npm run desktop
```

仅检查页面时运行 `npm run dev`，打开 `http://127.0.0.1:1420`。浏览器预览使用独立的内存工作台，不读写桌面数据库，不访问麦克风，也不模拟转写成功。

`npm run desktop` 会先构建实际 worker（macOS 启用 Metal，运行时可回退 CPU）。原生 worker 使用固定工程或应用包路径，不接受前端指定路径。首次进入“设置 → 本地模型”，下载后点击“使用模型”；收到 Ready 后才能开始听写。当前听写结果使用手动复制；关闭窗口只隐藏，退出应用才释放临时结果。macOS 默认按住 Control + Option + Space 听写、松开结束，也可从主窗口或菜单栏点击开始。设置 → 音频与快捷键支持修改、启停、恢复默认组合、保存和重启恢复；取消听写组合默认关闭。编辑期间暂停录音入口，注册或保存失败保留原配置；每个动作一个组合，Fn 单键暂不支持。该页也可重试注册/清理，并预览不会打开麦克风的悬浮条。物理键盘事件与外部应用占用仍需专项验收。

macOS 目标观察已加入运行进程的 Apple 签名检查、受签名保护的版本与二进制身份比较，以及 TextEdit 纯文本候选的有限结构检查。真实选区、持续观察及自动粘贴仍待兼容性验收；重现步骤及原生证据见 [目标身份验证](./target-identity.md)。

`npm run desktop:audit` 构建独立的只读目标验证应用，复用产品探测器验证 TextEdit 结构、选区、变更通知及取消；不会启动录音、模型、用户数据库或粘贴。原生用例、合成文稿及当前结果见 [目标观察验收](./target-audit.md)。它不替代自动输入的发布兼容性矩阵。

概览的听写准备原因与常见原生错误已使用固定代码提供中英文提示，覆盖模型下载/导入/切换、设备、快捷键、转写、复制与存储。错误保留结构，显示时按当前语言翻译；未知异常不直接显示底层正文。用量保存失败的提示不会被后续交付状态写入失败覆盖，已有结果仍可复制。合同、验证与剩余验收见 [原生错误提示](./native-errors.md)。

AI 服务设置可逐条处理异常配置，损坏记录不会阻断其他服务。可确认移除不可用配置并重新添加；无法识别的旧凭据保留提示，不猜测删除系统密钥。实现、恢复限制和独立合成 fixture 见 [AI 服务恢复](./providers.md#异常配置恢复)。

构建本机验证用的 macOS 应用包：

```sh
npm run desktop:build
```

产物位于 `target/release/bundle/macos/SpeakMore.app`，包含 `Contents/Resources/workers/speakmore-asr-worker` 与独立的 `speakmore-sensevoice-worker`。本地构建配置使用 ad-hoc 签名；不是 Developer ID 发布签名或公证产物，不能对外当成已验收安装包。正式分发还需可追溯 build ID、发布证书、公证、更新签名和干净系统验收。配置依据见 [Tauri macOS 签名文档](https://v2.tauri.app/distribute/sign/macos/)。

## 检查

数据库升级前自动生成并验证本地备份，所有待执行迁移在一个事务中提交。备份或迁移失败会进入恢复页；原文件保留，模型与听写服务暂不启动。支持查看固定数据目录及处理后重启，步骤和边界见 [本地数据恢复](./database-recovery.md)。成功升级后清理迁移备份；清理失败会显示可能仍有历史副本的提示。

本地模型也可离线导入：在未安装模型行点击“从本机导入”，按弹窗列出的文件名准备全部附件，再通过原生选择器一次选齐。文件只复制到私有模型目录，源文件与当前模型保持不变；大小、SHA-256 和所有附件通过校验后显示已安装，再主动点击“使用模型”。不支持自定义模型、压缩包或缺少 VAD/tokens 的文件。取消导入共用安装任务控制，清理完成后可重新选择。

`npm run test:models:import` 使用已经缓存的 Whisper / SenseVoice 公开文件，在临时目录完成离线导入、SQLite 重开、真实 CPU 转写和卸载；不下载文件、不使用麦克风、不写用户数据库。需要先准备既有模型/音频 fixtures 和两种 worker。

同一模型的新版本使用独立安装身份，应用升级保留旧目录条目、文件与选择；新版本安装后仍需主动使用，加载或选择保存失败恢复旧版本。保留版本可重新选择，停用后单独移除；当前生产目录仍为原有三项模型，没有新增未验收的权重。目录发布连续性和维护步骤见 [模型版本升级与回退](./model-catalog.md#模型版本升级与回退)。

```sh
npm run check
cargo check -p speakmore-desktop
cargo clippy --workspace --all-targets -- -D warnings
```

`npm run check` 覆盖 TypeScript/前端构建、浏览器工作台隔离、Rust 核心、模型目录验签/下载故障、音频帧、DSP/回调、无模型 worker、父进程与桌面 actor 故障注入 tests，以及听写操作/只读结果渲染、历史搜索竞态、跨页导出与失败保留目标文件。模型故障 tests 会监听短时本机回环端口，不访问外部下载服务。DSP tests 包含多采样率完整 5 分钟处理，debug 模式可能需要数分钟；不打开真实麦克风。真实 ASR tests 默认跳过，必须另行运行；这些检查不代表真机录音、跨应用输入、模型精度、签名安装或商业发布验收完成。

公开语料的准确率诊断使用固定 FLEURS / ASCEND 开发分区，通过打包 worker 与真实 VAD 计分，失败样本保留在分母。复现方式、数据归属及验收限制见 [识别质量诊断](./asr-public-quality.md)；语料只进入开发缓存，不包含在应用包中。

准备公开测试音频与源码锁定的模型（约 765 MB，保存在忽略的缓存目录），再运行真实 ASR tests：

```sh
python3 scripts/prepare-asr-fixtures.py --all-models
npm run test:asr
npm run test:asr:mac
npm run test:supervisor
npm run test:session
npm run test:session:asr
CARGO_TARGET_DIR="$PWD/target" cargo test --manifest-path vendor/whisper-rs/Cargo.toml owned_strings_survive_clone_and_free_after_last_owner --lib
```

真实 tests 包括 small/turbo 转写、5 分钟静音、5 分钟重复语音、48 kHz 双声道混音/重采样后转写、词典 tokenizer 预算、解码取消与复用、父进程 EOF，以及传输中断 watchdog。测试以 `--cpu` 启动 worker，不需要联网推理；fixture 下载脚本不进入产品。`resources/model-sources.lock.json` 保留上游来源记录；运行时使用 `resources/model-catalog.signed.json` 和独立的 `resources/model-trust.json`。两种 Whisper 规格各自包含完整的模型/VAD 文件包；SenseVoice Small INT8 使用独立 CPU worker，包含 ONNX 模型、tokens 和 Silero VAD v5，共 241.9 MB；已经通过真实下载、转写、取消及跨家族切换测试，质量/资源与真机 Gate 仍待完成。

模型管理验证：

```sh
npm run test:models
# 先构建 worker 并准备上面的公开 JFK fixture。此项实际下载约 191 MB，随后清理临时目录。
cargo build -p speakmore-asr-worker
npm run test:models:download
```

真实下载 test 使用桌面的 SQLite 适配器与生产 HTTPS 下载器：验证整个 bundle、关闭/重开数据库、重新校验本地文件、交给真实 worker 转写公开样本，最后卸载。它不打开麦克风，也不能替代桌面 WebView IPC 和用户点击流程的验收。

模型签名维护见 [模型目录维护](./model-catalog.md)。桌面构建会检查签名、字段合同与来源稿一致性；应用内没有接受未签名清单或任意前端 URL/路径的入口。

`test:session` 使用仅在测试 feature 中构建的音频/进程故障 fixture，覆盖完整 300 秒样本、取消、配置冲突、加载回退、存储故障和退出；还包含原生通知适配与中断代次、重叠休眠、旧请求隔离及恢复文件复核。Foundation 通知 tests 使用独立通知中心，不向 NSWorkspace 或系统广播伪造事件，也不令设备休眠；这些 tests 不证明真实识别精度或实际合盖/锁屏行为。`test:session:asr` 使用已验签的 Whisper small、真实 worker 和公开 JFK 音频穿过 actor，默认不打开麦克风。macOS 应用包构建后，可用测试专属环境变量 `SPEAKMORE_TEST_WORKER` 指定包内 worker，验证打包产物；产品构建没有这个路径覆盖入口。

设备查询与可选的真机采集检查：

```sh
# 只查询设备，不开启录音。
cargo run -p speakmore-audio --example devices
# 用户主动执行此命令才开启系统默认麦克风；最多录音 300 秒。
cargo run -p speakmore-audio --example capture -- --record-for 5
```

采集示例只在内存保留 PCM，输出时长/样本数/削波计数，不保存音频或联网。它不代替桌面权限、全局热键及输入交付验收。

## 工程边界

- `src/`：页面、受限 IPC 客户端、shadcn 基础组件。所有产品图标从 React Icons 导入。
- `crates/core/`：不依赖 WebView 的会话控制、配置、词典、历史与使用统计逻辑。
- `crates/protocol/`：有界二进制音频帧与控制消息，支持完整 300 秒音频。
- `crates/audio/`：cpal 设备协商、独占采集生命周期、2 秒有界 ring、16 kHz mono 重采样与 300 秒双重上限。
- `crates/asr-worker/`：私有 stdio 子进程、实际模型校验、Silero VAD、两个独立二进制的整段 Whisper / SenseVoice 解码与取消。
- `crates/sensevoice/`：由固定 C API header 生成绑定、原生对象 RAII、SenseVoice 与 ONNX VAD。
- `crates/models/`：Ed25519 模型目录、私有文件包、HTTPS 流式下载、取消/重试、原子安装与启动复核；加载租约保护模型文件。
- `crates/providers/`：四类文字整理服务的请求与响应适配、数据策略、2.5 秒期限、取消及原文保护；桌面设置、原生 Keychain 与配置事务已接入，会话整理已接通用户开关与所选服务，保留原文并提供固定回退原因。验证命令为 `npm run test:providers`，边界见 [AI 服务实现](./providers.md)。
- `crates/license/`：未接入的早期离线验签研究，当前桌面与 core 均不依赖；后续买断制另行设计。
- `crates/worker-supervisor/`：父进程握手、模型切换恢复、取消/超时、进程回收、连续崩溃熔断；由桌面 SessionController actor 驱动。
- `vendor/whisper-rs/`：0.16.0 绑定及明确记录的内存生命周期修复；底层 sys/whisper.cpp 保持锁定版本。
- `src-tauri/`：桌面运行时、唯一会话 actor、系统剪贴板与受限 commands；数据库在独立串行线程执行，完成后通过 actor 发布统一状态。
- `migrations/`：实际数据库迁移；发布后不得通过改写旧 SQL 迁移已有用户数据。

普通 Fn 听写的录音与协议上限为 300 秒、16 kHz mono f32、4,800,000 个样本；会议与实时字幕使用独立的连续采集流程。会议成果、跨会议问答、字幕回顾与助手的当前能力和验收边界见 [会议与字幕实现记录](./meetings-implementation.md)。本期不限每日次数，不设 Pro 门槛。历史默认关闭；API Key、音频与文本不得写入前端持久存储。

macOS 麦克风权限在“设置 → 音频与快捷键”显式申请，模型下载和快捷键不会弹出授权请求；授权后仍需主动开始。概览按模型、麦克风权限、设备顺序显示阻碍，并跳转到对应设置。运行中检测到麦克风权限撤销会取消未完成的听写，保留此前已生成的结果；已加入故障注入检查，真实 TCC 撤销/重新启动行为仍待验收。辅助功能缺失不影响手动复制，此版本不提前申请尚未启用的自动输入权限。

## SenseVoice 原生构建与验证

Apple Silicon 开发机运行 `python3 scripts/prepare-sensevoice-native.py --fixtures` 下载固定版本的 native 静态库和公开测试文件；`--offline` 只复核已有文件。`npm run desktop` / `desktop:build` 自动准备并独立构建两种 worker，端用户只下载模型数据，无需 Python、pip 或外部模型服务。

```sh
npm run test:asr:sensevoice
npm run test:sensevoice:native
cargo test -p speakmore-desktop --features session-fixtures --lib real_sensevoice_whisper_switch_through_actor -- --ignored --nocapture
cargo test -p speakmore-desktop --lib signed_sensevoice_download_sqlite_reopen_and_real_worker -- --ignored --nocapture
```

后两项使用真实原生模型；下载测试会在临时目录下载约 242 MB 并在结束清理。均只使用公开语音，不打开麦克风。SenseVoice 目前不使用个人词典，结果需人工检查；M4 上连续三次 5 分钟原生 decode 的默认配置测得单进程最大 RSS 2.185 GiB，低配机与整应用内存门槛尚未通过。分配策略对照见 [五分钟内存调查](./sensevoice-memory.md)；后续 [完整上下文注意力分块实验](./sensevoice-tiled-attention.md) 中，256 行候选的连续五分钟序列最大 RSS 为约 1.097 GiB，多语言序列为约 1.235 GiB，已测原生结果字段与基线一致；候选现已通过独立 `sensevoice-tiled256-v1` profile 的真实 worker/VAD 对照、取消、损坏文件回退和跨家族切换检查；详见 [worker 验证](./sensevoice-tiled-attention.md#完整-worker-验证)。后续已通过 [桌面会话导入、重启恢复与资源检查](./sensevoice-desktop-resources.md)：三次完整 300 秒及取消/切换流程通过，会话测试宿主与 worker 合计 RSS 采样峰值约 1.215 GiB。该范围不含 WebView 或真实麦克风，整应用内存 Gate 仍未通过；候选仅用于隔离开发验证，普通应用的受信下载目录尚未发布该版本。模型许可独立于 sherpa-onnx 代码许可，见模型设置与 `docs/model-catalog.md`。

worker 协议已为 v2，主程序与两种 worker 必须同次构建分发。数据库依次运行 migration 001–006，旧迁移保持不可改写；v1 旧结果默认待检查，新结果的质量提示和手动交付原因会保留至历史。migration 004 保存原生应用名称并合并大小写重复的 macOS 规则，保留更严格的限制。migration 005 增加 AI 服务名称和凭据清理日志；migration 006 增加可恢复的测试记录，旧时间戳保留显示但需重新测试。升级关闭文字整理，不修改模型和转写数据。

“设置 → AI 服务”支持添加、编辑、删除、测试和选择 OpenAI、Anthropic、Ollama 与兼容接口，最多 20 个配置。API Key 存 macOS Keychain，SQLite 只存原生 reference；轮换失败保留旧配置，未完成清理可重试。配置期间阻止新录音，关闭页面不放弃已经开始的保存事务。本机 Ollama 使用已自行安装的本地服务；Whisper / SenseVoice 的下载和选择仍位于“本地模型”。连接测试只发固定句，成功后保存与配置和凭据版本绑定的记录，重启后可在使用时恢复且不重发请求；写作模式中可单独开启整理，原文先保存并计次，整理只更新同一条结果；不能据此声称商业验收完成。

本期功能正常开放：听写不限次数，单次最长 5 分钟，所有已发布本地模型与 AI 整理无 Pro 门槛。关于页展示本期能力，不提供订阅、购买、升级或付费激活。AI 服务仍需配置和测试，云端请求使用用户自己的服务凭据。后续计划买断制，价格、权益和履约方案另行确定。

“设置 → 应用规则”现支持通过 macOS 原生窗口选择 `.app`，添加、编辑和移除“始终手动粘贴”“不保存该应用历史”两项规则。规则由原生选择凭据创建，共用设置 revision，支持最多 100 个应用；不加载所选应用的可执行代码。快捷键和菜单栏录音冻结开始时的来源，主窗口录音使用 SpeakMore 自身的规则。录音中收紧规则会保留到本次结束，关闭/删除只影响下一次；来源未知且配置了历史排除时不保存本次历史。原生自动输入兼容性资格仍独立控制。

数据库现增加 migration 003，保留原记录与质量标记，并为新结果保存固定的手动交付原因。结果详情、历史和 TXT/CSV 导出可解释未自动输入的原因。macOS 已接入单线程 AX 候选观察、应用激活代次和焦点/选区变化失效；当前仅有 TextEdit 候选适配，真实 AX 兼容矩阵尚未通过，自动粘贴仍未启用。目标局部文字只用于内存摘要，不发送到前端、不写入数据库。

ASR 结果接受后另行请求原生目标复核，之前的心跳不能替代该次读取；复核最长 300 ms，不阻塞取消和结果保留。结果复制与自动尝试共用一个常驻写入线程和唯一槽位；macOS 使用完整文本 item、私有标记和 changeCount 复核，超时/取消不启动替代写入或自动重试。旧剪贴板内容不读取、不恢复。系统调用开始后的超时会说明复制结果未知，线程返回前保持忙碌。

自动交付已接线修饰键释放等待、写入前后独立目标复核、一次 Cmd+V 提交和同一历史行的条件状态更新。提交前先保存 Uncertain；完成事件投递只记为 Submitted，不声称目标已接收。历史关闭或记录已删除时不新建记录，重启不会重放。最终状态仅在成功保存后同步到已打开的历史页，保留详情选择与分页。原生目标兼容性资格目前全部关闭；该链路已通过合成平台故障注入，仍需真实应用矩阵验收后才能对用户启用。

原生剪贴板专项检查使用独立临时命名剪贴板，仅写合成文字，不访问系统通用剪贴板：

```sh
npm run test:clipboard:mac
# 只构造/检查 Command+V 的四个原生事件，不向系统投递。
npm run test:input:mac
```

这些命令需要已登录的 macOS 桌面及相应原生服务访问权限。两项临时剪贴板测试和一项事件构造测试在常规套件中默认忽略，须显式执行；普通故障注入 tests 覆盖并发、排队过期、调用方取消、不可中断调用的槽位占用、剪贴板覆盖与不确定提交。原生测试通过也不能替代实际结果按钮、系统剪贴板及跨应用粘贴验收。

独立模型界面验证与多版本操作：[原生模型管理检查](./model-ui-check.md)。

公开五分钟音频的真实 GUI/IPC、取消恢复与 WebKit/XPC 归属采样：[macOS 原生界面资源检查](./macos-gui-resources.md)。M4 本次候选合计 RSS 采样峰值 1.528 GiB；物理采集、最低机型与连续 30 分钟 Gate 仍待完成。

模型安装阶段的真实进程终止与限容 APFS 磁盘写满检查见 [文件系统故障验证](./macos-filesystem-faults.md)。五项通过，旧模型和已有数据保留；数据库事务内部中断与安装器升级仍需验收。
