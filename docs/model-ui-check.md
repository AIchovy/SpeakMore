# 本地模型的原生界面验证

2026-09-16。使用独立的 `SpeakMore Model Check` macOS 应用，检查原生文件选择器、实际 WebView/IPC、模型管理、SQLite 及打包 worker 的组合行为。没有替换前端返回值、直接调用隐藏 IPC 或通过写数据库模拟安装/选择。

## 构建与隔离

```sh
CARGO_NET_OFFLINE=true npm run desktop:model-check
```

此命令仅面向 Apple Silicon macOS。构建需要现有锁定依赖和原生 worker 输入；离线模式不下载依赖。输出为 `target/release/bundle/macos/SpeakMore Model Check.app`，应用标识固定为 `com.speakmore.modelcheck.20260916`。普通用户版和冻结的 Input Check 包保持独立。

`model-check` feature 仅在此验证应用中载入内置测试清单；使用该 feature 的可执行文件遇到其他应用标识立即拒绝启动。测试清单由公开的固定 fixture seed 签名，包含原有三项条目与独立版本候选。原生仍验证签名、旧条目连续性、模型 profile、大小和 SHA-256。生产信任根拒绝此测试清单。没有环境变量、任意路径、URL 或 IPC 清单覆盖入口，也没有读取生产私钥。

候选为 `sensevoice-tiled256-v1` / `tiled256-v1`，安装 ID `sensevoice-small-int8--449f53e709f54de54d48c2bbfa225bd5d2f69e9ad620f78c946cd69de9dc16d2`。候选清单中的 URL 仅为测试占位；验证使用本机导入，不能当作可发布的下载地址。普通应用的三项生产目录未增加候选。

应用使用独立 Application Support 目录。首次启动前确认目录不存在；启动准备关闭这一个验证应用的听写/取消快捷键，交付设为手动，不改变当前模型。准备逻辑重复执行不递增已符合条件的设置 revision。没有申请系统权限、打开麦克风、添加云端服务或生成听写结果。

## 原生操作

模型源文件先复制到开发缓存的三个独立文件夹并逐项核对固定摘要：原版 SenseVoice、256 行候选和 Whisper small。通过真实 NSOpenPanel 选择完整文件集，路径只由原生导入接口处理。原文件未移动或修改。

| 检查                 | 结果                                                                                              |
| -------------------- | ------------------------------------------------------------------------------------------------- |
| 初次打开             | 四个清单项均未安装；概览要求先选择模型                                                            |
| 原版 SenseVoice 导入 | 显示复制进度、校验和已安装；没有自动选中                                                          |
| 使用原版             | 准备期间禁用其他模型操作；Ready 后保存原版，显示 CPU                                              |
| 导入候选             | 原版继续显示当前/就绪；候选独立安装，设置 revision 保持 4                                         |
| 使用候选             | Ready 后候选显示版本标签/当前模型/CPU；原版仍可选择；revision 5                                   |
| Whisper small 导入   | 候选仍为当前模型；Whisper 独立显示已安装                                                          |
| 切换家族             | Whisper small 实际显示 Metal 就绪，revision 6；切回候选 CPU 就绪，revision 7                      |
| 使用中删除           | 当前版本的移除按钮禁用；未执行图形界面删除                                                        |
| 实际退出重开         | 确认应用不再运行后重新启动，经历文件复核并恢复候选 CPU Ready；revision 仍为 7，三个安装登记均保留 |
| 弹窗关闭             | 新构建的中文关闭按钮实际读作“关闭”；点击后焦点回到对应的 turbo 导入按钮                           |

[候选 Ready 的原生截图](./screenshots/macos-model-candidate-ready.jpg) 显示原版与候选并存，以及仅候选的当前/就绪状态。[退出重开后的模型列表顶部](./screenshots/macos-model-candidate-reopened.jpg) 已复核。重开时替换的是同标识的本机构建（仅补齐弹窗语言标签），不是正式安装器升级或崩溃恢复。状态文本来自 WebView 的实际辅助功能树；SQLite 只读核对安装身份与选择修订，未用于驱动操作。

文件选择器中，Computer Use 的图标/行点击和 Command+A 没有产生可靠的选择。改用原生列表视图与 Edit → Select All，确认完整文件集实际选中后再点击导入。该结果验证原生菜单路径，不能声称物理键盘多选或全部选择器交互已通过。

## 界面修复

多个版本原来具有相同的下载/导入/移除辅助功能名称，使用/停用/重载也缺少模型上下文。现在这些名称和取消操作均包含模型名与版本；原版仍沿用不带版本的名称。实际原生树能区分 `使用模型 SenseVoice Small` 与 `使用模型 SenseVoice Small · tiled256-v1`，不会仅依赖视觉上的版本徽标。

中文模型导入弹窗的关闭按钮原先读作 `Close`。已为导入、应用规则、历史导出、快捷键、命令搜索和帮助弹窗传入当前语言标签；既有词典、服务、许可和云端同意弹窗保留相同做法。此修改不改变关闭/取消或确认事务的行为。

## 验证边界

本轮不产生语音转写，不计入质量、五分钟性能、物理麦克风或端到端取消验收。前一轮 [桌面会话资源检查](./sensevoice-desktop-resources.md) 的内存数据仍仅包含测试宿主和 worker；没有在本轮把它重新解释为整应用占用。后续的独立 [公开音频 GUI 资源检查](./macos-gui-resources.md) 已归属 WebKit/XPC 并同步采样，保留主进程 300 MiB、整应用 2 GiB、连续 30 分钟和最低机型要求。

没有发布新模型、签发生产目录、安装正式更新或验证崩溃/断电恢复。GUI 删除、实际录音/权限、自然语料质量、VoiceOver/200%/多屏、可信模型发布源及其他商业发布 Gate 继续保持未完成。

## 本轮检查与证据

前端 46 项既有回归通过；TypeScript/Vite 构建通过。新增两项原生检查通过，覆盖测试清单连续性、候选精确身份、生产信任拒绝测试清单，以及启动准备的幂等性。桌面 all-targets Clippy（`session-fixtures,target-audit,model-check`，`-D warnings`）与 Rust fmt 通过；没有将默认忽略的真实 ASR tests 计入本轮。

普通应用和最终 Model Check 包均构建成功，release 编译各约 1 分 33 秒；deep/strict codesign 验证通过，均仍为本机 ad-hoc 签名，未公证。普通应用标识为 `com.speakmore.desktop`、最低 macOS 14.0，主程序与两个 worker 为 arm64，未含测试清单信任、测试应用标识或私钥文件。生产签名目录与发布基线逐字节相同，冻结 Input Check 二进制摘要未变。

[结构化记录](./evidence/model-ui-check-2026-09-16.json) 包含两次验证构建身份、普通包/worker 摘要、原生界面观察、截图摘要与最终数据库只读核对。首次七个阶段与重开后的三个阶段明确分属两次构建。最终 SQLite 为 v7 / integrity ok，三个模型登记的八个文件均匹配受信摘要；选择 revision 7，历史、用量、服务和词典均为 0。验证应用已确认退出，普通用户应用未启动。
