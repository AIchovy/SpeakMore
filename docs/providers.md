# AI 文字整理服务实现

`crates/providers` 提供四类服务的原生请求适配与有界网络执行：OpenAI、Anthropic、Ollama、OpenAI 兼容接口。它与“设置 → 本地模型”中的 Whisper / SenseVoice ASR 分开；没有音频、历史、目标窗口、数据库、Keychain 或 WebView 访问能力。

当前已接通“设置 → AI 服务”、原生配置与系统凭据事务、连接测试和选择。macOS Keychain 已用全新随机账户下的合成凭据验证保存、读取、删除与删除确认。本期无付费资格要求；**会话整理已接入**，可在写作模式中单独开启；原文先保留并计次，整理更新同一条结果。不得通过前端 `tested` 布尔值绕过服务验证；合成服务测试不等于真实账户、模型质量或商业发布验收。

## 桌面配置与凭据事务

主窗口专用 IPC 为 `get_providers`、`save_provider`、`test_provider`、`cancel_provider_test`、`select_provider`、`delete_provider`、`retry_provider_cleanup`、`discard_invalid_provider`。最多保存 20 个配置，名称、完整 endpoint、精确模型 ID、协议 profile 与同意的 origin 存 SQLite；前端只能取得凭据是否配置，不能读取 Key 或传任意 Keychain 账户。兼容接口显式选择预算参数与指令角色，其他协议使用固定 profile。

密码框不使用 React state 或浏览器持久存储；提交前同步清空 DOM 字段，Rust 将密钥立即包入 `SecretKey`。这不保证 IPC、WebView 与 HTTP 库的所有内存副本都已清零。保存失败保留非敏感草稿，密钥需要重新输入。

`src-tauri/src/providers/vault.rs` 使用一个常驻线程和单槽位执行系统凭据操作。macOS 明确构造 `MacCredential` 的 User 域，service 固定为 `SpeakMore`，account 为两个原生生成的 UUID：`provider UUID / credential revision`；不依赖可被 mock 替换的全局默认 builder。Windows target 已选择原生后端，但未通过 Windows 验收。

数据库 migration 005 增加显示名称和 `provider_credentials_pending` 日志表；后者只有 reference、provider ID、用途及时间，没有密钥字节。更换密钥顺序为：先持久化 candidate → 写入新 Keychain 条目 → 在一个 SQLite 事务中切换 reference 并登记旧条目清理 → 清理旧条目。保存冲突或写入状态不确定时，先查询数据库权威引用，再处理候选条目，保留旧配置。删除配置先取消选择并关闭整理，再登记凭据清理；清理失败保留重试入口，不显示“已完全清除”。

凭据读取与删除错误按固定类别映射，不回传系统错误对象；删除后必须确认 `NoEntry` 才删除日志记录。中断或崩溃留下的日志在设置中显示；后续保存、删除或用户主动重试时执行清理，启动不会直接弹出钥匙串授权。日志上限 64，超限必须先处理清理。

每项变更/测试取得会话 actor 的原生配置预约；录音中拒绝配置，配置中拒绝新录音，期间按住的快捷键不会在完成后突然开始。设置与 provider 各自修订号均在事务内复核。关闭 WebView 或丢弃 IPC 等待不会放弃已接收的保存事务；系统凭据调用仍在执行时维持占用，不重复启动另一项。系统交互无法强制中断，界面如实显示等待并允许用户处理系统提示。

测试只在用户点击时发送固定句，先验证本地策略和目标同意，再读取凭据。测试通过不自动选择或开启整理；取消不会让迟到成功变成可用资格。成功测试现保存与配置和凭据版本绑定的记录，重启后可在原生端重新验证并恢复，无需重发测试句。读取设置页和启动不读 Keychain；用户明确选择使用时才读取所绑定的系统凭据。实际会话使用开始时冻结的服务配置、测试记录和隐私策略。

## 异常配置恢复

读取服务按行校验，损坏的 ID、类型、UTF-8、过长字段、凭据引用或协议不会使健康服务的列表、编辑与删除一起失效。SQL 先检查类型和字节长度，再把有限字段复制到 Rust；原生层继续验证地址、协议和凭据归属。异常行只显示可用的短名称与固定说明，不把原始协议、坏地址或凭据 reference 传到 WebView。SQLite 查询失败、文件损坏和磁盘错误仍返回实际存储失败，不能当成空列表。

“需要处理的配置”支持移除不可用服务并重新添加。确认对象由原生进程持有，前端只收到不透明 UUID，不接受 rowid、表名、Keychain 账户或任意 SQL。对象绑定当前 Store 实例、连接内修改计数及同一连接的 `PRAGMA data_version`，列表最后读取后 5 分钟过期；写事务先取得 IMMEDIATE 锁，再核对变更状态与 settings revision。外部连接修改、同连接修改、重启、过期或已消费对象均需重新读取列表并确认。此机制用于防止陈旧操作，不能当作抵御数据库被恶意替换的安全边界。[SQLite data_version](https://www.sqlite.org/pragma.html#pragma_data_version)、[total_changes](https://www.sqlite.org/c3ref/total_changes.html)。

移除被选中的异常服务时，同一事务取消选择并关闭文字整理；只影响该配置。模型、历史、其他服务和有效的持久测试记录保留，内存准备缓存重新验证。有效且 provider ID 匹配的凭据 reference 进入原有清理流程；无法辨认或归属不匹配的 reference 原样保留到已有本地清理日志，不交给系统钥匙串执行。界面持续显示未确认清理状态，并提供“忘记此记录”：这个动作只删除本地无效日志，不操作或声称删除系统密钥，确认框解释须自行检查 SpeakMore 钥匙串项目。读取列表及显示恢复入口不读钥匙串、不发送服务请求。

异常服务仍计入 20 条上限，不能通过损坏记录绕过限制。每类列表最多读取 100 条；超出时显示数量异常与刷新提示，不自动批量删除。原有 64 条清理记录容量约束继续生效。所有恢复动作与录音、服务测试和凭据操作互斥，已接收的事务不随窗口关闭放弃。数据库仍为 v7，无迁移。

开发 fixture：`cargo run --offline -p speakmore-core --example provider_recovery_fixture -- /new/path/data/speakmore.sqlite3`。拒绝覆盖文件，创建一份正常服务、两份异常服务和一条无法识别的清理记录，不创建实际钥匙串条目，并关闭合成实例的默认全局录音快捷键。使用独立应用标识时，目标必须是该实例应用数据目录下的 **data/speakmore.sqlite3**。

## 跨重启测试记录

migration 006 增加 `test_checkpoint`，格式为 `v1.` 加 64 个小写十六进制字符。SHA-256 输入有独立域和逐字段长度，绑定归一化完整 endpoint、精确模型、协议参数，以及原生 provider ID、配置 revision、不可变 credential reference。这里不包含 API Key 或密钥摘要；标记是本地配置一致性检查，不是服务商签名、加密记录或防止用户篡改数据库的认证。

只有成功显式测试返回的 `TestReceipt` 才能为其原始 PreparedProvider 生成记录，主窗口 IPC 不接受测试记录或“已测试”布尔值。SQLite 的测试时间和记录一起提交。v5 的旧时间戳保留用于展示，但没有新记录，不能获得可恢复资格；升级关闭文字整理。旧迁移不改写，该检查点使用 v6；当前数据库为 v7，新增整理回退原因。

“使用”操作先在原生端校验修订、完整配置、当前处理策略、目标同意与保存记录，再读取该记录绑定的系统凭据；成功后将不可变配置、凭据和内存 TestReceipt 保存在一起。相同修订的已准备配置可复用，修改任一配置或凭据版本须重测。保存、新密钥替换、明确移除密钥、删除服务和重测会使旧资格失效。单纯重启不自动发请求、读取钥匙串或授予云端同意。

重测开始前先原子撤销旧记录并关闭该配置的整理开关。撤销事务失败则不进入网络测试，原有效记录按事务语义保留；撤销已提交后，失败或取消不能在重启后恢复旧通过状态。保存新结果前是取消接受的截止点：原生状态锁内检查取消并切换到“正在保存测试结果”，随后释放锁再执行 SQL，此时不再提供取消入口；不在磁盘或 OS 调用期间持锁。保存失败不发布内存资格。界面分别展示“尚未测试/重新测试后可使用”“测试记录有效，使用时准备”“本次运行已准备”，避免把历史时间戳或磁盘记录当成本次已读取 Keychain。

当前尚未完成真实云端账户的跨重启、质量与凭据库完整交互验收。可恢复的连接测试不保证模型永久可用或文字整理质量合格。

## 实际会话整理

“写作模式 → AI 文字整理”提供独立开关。开启在原生配置预约内检查当前设置修订、所选服务、测试记录、处理策略和凭据，在提交开关前再次核对修订。没有有效服务或正在听写时不能开启；通用 settings IPC 仍拒绝直接提交 `polishEnabled=true`。关闭使用普通设置事务，允许在听写期间执行。

开始录音时冻结模式、自定义要求、provider 的配置与不可变凭据引用和隐私监视对象。ASR 结果被接受后，立即进入内存缓存并排队保存原文与唯一用量事件；整理请求与这次存储并行，交付等待二者完成。凭据准备、请求、响应读取和检查共用外层 2.5 秒预算，磁盘事务不计入网络等待承诺。不重试、不换服务、不截断原文；需要复核的 ASR 结果跳过整理。

整理完成只更新已有记录的 `polished_text`、`polish_outcome`、`polish_ms` 和受枚举约束的 `polish_reason`，不再计次。删除的历史、已撤销的历史同意、应用历史排除不会被结果返回恢复；清空内存面板也不会被迟到完成重新填入。原文保存失败仍保留缓存并尝试一次用量记录，失败不会因整理自动触发输入。结果最终按现有目标复核与一次交付规则处理。

成功关闭整理会在存储提交边界取消所有在途整理；切回仅本机只取消远程整理。本次监视对象的取消不可逆，重新开启只影响后续会话。取消/中断/退出与结果接收在原生状态上核对；已发送文字无法撤回。任务随会话持有并在释放时取消，不产生后台重试或稍后自动替换文字。

migration 007 为历史新增固定回退原因，保留原有记录和设置。结果详情用中英文说明服务未就绪、策略撤销、取消、超时、文字较长、需要复核、无效要求、连接失败或响应不合格。历史页在会话结束后刷新，正在查看的结果不因刷新先清空；旧分页不能覆盖新结果。

## 调用边界

1. `Configuration::validate` 检查完整 endpoint、协议和精确模型 ID。只允许 HTTPS；只有 Ollama 的字面量 `127.0.0.1` / `[::1]` 可使用本地 HTTP。不补路径，不接受 userinfo、query、fragment、空路径或零端口。十进制、十六进制、简写 IP 及 localhost 不获得本地策略豁免。
2. 原生设置保存明确同意的 origin，测试时复核后为此次不可变配置生成 `DestinationConsent`。新配置不能复用另一对象的确认；同一不可变配置的克隆可以使用。它不是可由 IPC 反序列化的权限字段。服务类型或 origin 变化需再次确认并重新输入或明确移除旧密钥。
3. `PreparedProvider` 持有不可变配置及其凭据。`SecretKey` 校验长度和 header 字符，持有 `Zeroizing<String>`，不实现序列化，Debug 隐去内容；HTTP 认证 header 标为 sensitive。HTTP 库会持有 header 副本，这不等于承诺清零整个进程内所有副本。
4. 显式 `test_connection` 只发送固定合成句，校验对应协议、完成标记和输出，成功返回不可反序列化的内存 `TestReceipt`。不发送当前或以前的转写。凭据轮换使旧测试记录失效；新的进程实例可在核对持久记录及不可变凭据引用后恢复内存凭据，恢复不会发送请求。
5. `polish` 检查原生开关、测试凭据、数据策略、取消及输入长度后才发送。原生 controller 必须从已保存的开关构造启用状态，同一会话最多调用一次。`Client` 的克隆共用一个请求槽位，同时覆盖测试与润色；controller 应持有一个共享 Client，不能按请求新建实例绕过占用。
6. 成功只返回最终文字；失败只返回 `Original(Failure)`，调用方继续持有完整原文。actor 已接入原文保留、一次用量、历史关闭/删除、取消、迟到结果隔离和一次交付规则。

本地模式指请求仅到字面量回环地址，不能证明用户自行运行的 Ollama 服务内部没有再代理到外部。这是用户管理的独立服务，不属于 SpeakMore 的 ASR worker；SpeakMore 不自动启动服务、拉取 Ollama 模型或改变其配置。

## 请求与输出

官方 OpenAI 适配使用 `developer` 指令、`max_completion_tokens=2048`、`stream=false`、`store=false`，不固定具体模型名称。较旧模型或兼容服务可显式选择兼容 profile 的 `system` / `developer` 和两种预算字段，再主动测试；听写失败后不会猜参数重发。Chat Completions 是本产品跨服务适配的选择，`store=false` 不代表服务商零留存。[OpenAI 官方接口](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)

Anthropic 使用顶层 system、固定 API 版本、非流式 Messages；只接受文本块和自然结束。当前不发送 stop_sequences，因此 stop_sequence 不算完整结果。`stop_details.type=refusal` 即使同时出现 end_turn 也返回原文，不回传其中的解释。[Claude 官方接口](https://platform.claude.com/docs/en/api/messages/create)

Ollama 显式关闭默认流式返回，设置 `options.num_predict=2048`，要求 done=true、done_reason=stop 和 assistant 文本。没有请求工具、图片或音频；返回工具调用或媒体时不采用该结果。[Ollama 官方接口](https://docs.ollama.com/api/chat)

原文和编辑指令是分开的消息。标准、邮件、代码说明和自定义模式保留统一规则；自定义说明最大 4 KiB。原文最多 4,000 Unicode 标量，超出时不截断、不分段请求；输出非空、最多 16 KiB，拒绝 NUL 和非换行/制表符的控制字符。

输出保护检查比较数字、HTTP(S) URL、邮箱、反引号片段及其出现次数。数字连同紧邻的正负号、百分号/千分号保留；删除重复值、改变数字、增加数字或修改受保护片段都会退回原文。URL 末尾句读与多余闭括号作有限区分。这个保守检查不能证明否定词、专名或整句语义没有变化，真实语料质量评估仍必须完成。

## 网络执行

整个调用共用 2.5 秒 deadline，包括发送、读取、解析与文本检查；不为每个 chunk 重置期限。关闭 HTTP 重试、所有重定向、自动 Referer、代理和自动解压，不保留空闲连接以复用旧请求。云端也使用直接连接，当前不提供代理配置。

只接受 HTTP 200 的 application/json（可带 charset）。响应通过读取过程限制为 256 KiB，同时检查 Content-Length；没有长度头也不能越界。显式拒绝压缩编码。JSON 关键字段重复、非文本、错误字段、拒绝、工具调用、截断和不兼容完成状态都不会产生替换文本。

取消、deadline 和丢弃调用 future 都会结束等待、释放本模块请求槽位。HTTP 库可能继续清理内部 DNS/连接任务，已发送的内容无法撤回，远程推理也不保证立即停止；迟到响应没有写入结果、历史或重新发请求的入口。错误仅含枚举诊断码与必要 HTTP status，不保留上游错误对象、URL、header 或正文。

## 验证与后续接入

```sh
npm run test:providers
cargo clippy -p speakmore-providers --all-targets -- -D warnings
cargo test -p speakmore-core
npm run test:session
npm test
# 仅创建并删除一个全新随机账户下的合成测试凭据；需要 macOS 原生钥匙串访问。
cargo test -p speakmore-desktop --features session-fixtures --lib providers::vault::tests::native_keychain_synthetic_roundtrip_and_verified_removal -- --ignored --exact
```

测试只使用合成文字/凭据和随机本机回环端口。四种 wire profile 通过同一个 HTTP 传输实现验证；云端协议的 HTTP 替换仅存在于 `cfg(test)` 模块，发布构建没有不安全 endpoint 开关。另行验证 TLS 握手失败不会退回明文 HTTP；尚未验证真实云端证书链、账户、模型兼容性、延迟或整理质量。

2026-09-15 验证：核心 42 项通过，前端 30 项通过；桌面常规 71 项通过（含新增的事务、策略和取消集成测试），另有 9 项需显式运行的原生/模型专项，其中真实 Keychain 专项已单独通过。测试使用临时数据库和合成凭据。独立 bundle `com.speakmore.providerscheck.20260915c` 中已检查本机服务添加、保存后焦点返回、失败提示、未测试时禁用选择，以及中英文和深色界面；未读取已有用户密钥。

上述初期验证之后，actor 润色、取消/收紧策略与一次结果交付已接通，见下方会话接入检查点。真实云端账户、OS 权限拒绝/锁定的完整交互、所有分辨率和辅助技术仍需专项验收。

2026-09-15 跨重启检查点：providers 21 项、core 49 项、前端 34 项、desktop 常规 73 项通过（另 9 项按需专项忽略）；对已有成功记录进行重测后取消的补强用例也单独通过。覆盖配置/协议/地址/凭据版本绑定、旧时间戳不能授权、v5 → v6 保留和完整回滚、持久撤销失败的原子性、无第二次网络请求的恢复，以及失败/取消重测后不能恢复旧资格。

独立 macOS 应用 `com.speakmore.providerrestorecheck.20260915e` 中，用户界面实际添加无密钥本机服务并完成一次固定句测试；临时服务器确认仅处理一次请求后退出。应用退出重开后显示[测试记录有效](./screenshots/macos-provider-test-restored.jpg)，服务器已停止时点击“使用”仍恢复为本次已准备。随后主动重测失败，界面显示[失败且旧资格撤销](./screenshots/macos-provider-retest-revoked.jpg)，独立数据库中 checkpoint/tested_at 均为空，保留仅本机策略、关闭整理，使用与历史均为零。成功测试时的[已准备状态](./screenshots/macos-provider-test-prepared.jpg)也已留存。此项没有测试真实云端账户、密钥跨重启、麦克风或整理质量。

2026-09-15 会话接入检查点：core 52 项、providers 21 项、前端 38 项、desktop 常规 75 项通过（另 9 项按需忽略）。真实 actor 与合成本机服务验证先保存原文、一次记账、模式冻结、成功整理和取消/隐私/删除/超时/退出回退；退出竞态修复后完整回归通过。独立 macOS GUI 检查免费开关、配置导航、英文深色与重启恢复，未使用真实云端账号或生产许可。完整证据与仍待完成的商业验收见 [实施记录](./implementation-status.md)。

2026-09-16 异常配置恢复检查点：core 56 项、前端 40 项、desktop 常规 82 项通过（另 11 项按需忽略），all-targets clippy 通过。覆盖逐行隔离、字段长度/类型/UTF-8、无效行计入容量、凭据归属、同连接/外部修改与重开后的陈旧确认、事务回滚，以及真实 actor 中录音互斥和五分钟确认过期。独立原生应用实际显示健康与异常行并检查两种确认框、取消焦点恢复和英文深色布局；GUI 未提交移除，实际移除事务由自动测试验证。截图：[中文列表](./screenshots/macos-provider-recovery.jpg)、[忘记记录确认](./screenshots/macos-provider-recovery-confirm.jpg)、[英文深色服务列表](./screenshots/macos-provider-recovery-en-dark-services.jpg)。测试数据均为合成配置，不含实际系统密钥；没有录音、云端请求或系统权限变更。物理数据库损坏、磁盘故障和真实 Keychain 锁定/拒绝仍需专项验收。
