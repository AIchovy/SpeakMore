# macOS 目标身份验证

此模块为通用自动粘贴提供进程身份基础，所有应用均绑定 PID、启动时间及 bundle ID，拒绝 SpeakMore 自身。TextEdit 额外保留 Apple 签名验证。下文的签名步骤与早期验收记录只针对 TextEdit，不代表当前仅支持 TextEdit；当前通用目标流程以代码实现为准。

## 实现

`src-tauri/src/target/mac/identity.rs` 在唯一原生观察线程执行：

1. 检查有效 PID、运行状态、启动时间与 TextEdit bundle ID 提示。
2. 每次使用 `SecCodeCopyGuestWithAttributes` 向内核取得当前 PID 的动态代码对象。固定要求是 `identifier "com.apple.TextEdit" and anchor apple`，不接受前端或目标应用提供的要求字符串。
3. 调用 `SecCodeCheckValidity`，显式禁用联网校验；失败即停止，不读取目标控件。
4. 成功后调用 `SecCodeCopySigningInformation`，核对签名标识、受签名保护的 Info.plist 中的 bundle ID、版本与 build，以及二进制唯一签名标识。字符串和数据类型、长度均受限；没有版本或签名标识即不能继续。
5. 再次查询运行实例，确认启动时间未变；将进程和代码身份冻结进 TargetSnapshot。初始观察与后续每次复核都先比较身份，再读取控件、选区和有限上下文。

使用 `NSRunningApplication.bundleIdentifier` 仅能预筛候选；复制名称、bundle ID 或版本不能通过固定的 Apple 签名要求。签名信息复制 API 本身不验证真伪，必须在同一代码对象上先完成有效性检查。[Apple 签名信息 API](<https://developer.apple.com/documentation/security/seccodecopysigninginformation(_:_:_:)>)

版本来自 Security 所见的受保护 Info.plist。代码唯一标识可区分使用相同版本号的不同签名二进制；只作内存比较，不假定其固定为 SHA-1，也不向前端输出。原生对象和签名信息均由 RAII 释放，不写入数据库或日志。这里的身份验证不证明目标进程的全部运行行为或插件可信。

## 时限与降级

身份检查与 AX 探测共用 300 毫秒截止时间，包含排队等待。每个原生调用前后检查时限，超时优先返回 `ProbeTimeout`；签名要求不匹配返回 `UnsupportedTarget`，信息无法确认返回 `TargetUnavailable`，会话期间身份变化返回 `TargetChanged`。结果供用户复制。

Security 同步调用不能由本模块强制取消。超时后的系统工作仍占用原有唯一观察线程，返回值不再用于该会话；不会增加线程或重试。`kSecCSNoNetworkAccess` 在首发 macOS 14+ 基线上用于动态代码验证，避免身份检查产生网络请求；在线吊销与公证检查不在该验证结论中。签名校验仍可能因本机缓存、磁盘或系统服务变慢；单台机器的时延不能作为发布性能保证。

## 验证方法

常规回归覆盖元数据类型和长度、空值、控制字符、错误标识、签名数据变化、过期预算和非法 PID，并验证当前测试进程无法满足 TextEdit 的 Apple 签名要求：

```sh
cargo test --offline -p speakmore-desktop --features session-fixtures --lib target::mac::identity
```

两个按需原生检查使用真实 Security API：

```sh
# 先自行打开系统 TextEdit；测试不启动应用、不读取 AX 或文稿。
cargo test --offline -p speakmore-desktop --features session-fixtures --lib target::mac::identity::tests::native_running_textedit_identity -- --ignored --nocapture --test-threads=1

# 复制 /bin/sleep 到临时目录并以同一 bundle 标识重新作 ad-hoc 签名。
# 只启动该临时子进程；退出/断言失败均终止并回收它，随后清理临时目录。
cargo test --offline -p speakmore-desktop --features session-fixtures --lib target::mac::identity::tests::native_adhoc_claiming_textedit_is_rejected -- --ignored --nocapture --test-threads=1
```

2026-09-15，本机 macOS 15.6.1 / Apple M4，实际运行的 TextEdit **1.20 / build 409** 通过校验：首次 **100.23 ms**，随后 12 次复核最大 **2.45 ms**，每次使用 300 ms 预算；这是身份模块观测，不含 AX、用户录音或粘贴。冒用 `com.apple.TextEdit` 的临时 ad-hoc 进程被固定 Apple 要求拒绝。

2026-09-16 最终补充应用自报 bundle ID 的复制前长度限制后，包含两项按需原生检查的 **5 项身份测试全部通过**。TextEdit 同版本复测初次 3.39 ms、随后 12 次最大 2.60 ms；此次已有系统缓存，不能当作冷启动保证。

受限命令沙箱中 LaunchServices 枚举返回空列表；只读进程检查经自动审批后在正常登录会话环境完成。未修改辅助功能权限、打开麦克风、读取目标文稿或写入系统通用剪贴板。非 TextEdit 的大型测试二进制在一次冷校验中超过 300 ms；拒绝要求的专项测试使用 5 秒测试预算以单独验证签名拒绝，生产时限仍为 300 ms，超期只能手动交付。

## 尚待完成

TextEdit 已增加有限角色和双向父子身份检查，在读取选区前拒绝格式工具栏、标尺和未知结构；实际合成文稿观察及独立验证应用见[目标观察验收](./target-audit.md)。结构候选尚未通过完整原生观察矩阵，不能因签名和形状匹配而获得自动输入资格。

浏览器标准 input/textarea 与富文本区别、VS Code 普通编辑器适配、版本化兼容性清单和完整 I01–I09 / C01–C02 真机矩阵尚未完成。签名通过、模拟目标测试或单次时延均不能替代这些验收。其他平台需要独立实现身份与权限策略。
