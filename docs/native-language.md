# macOS 语言初始化与原生菜单

2026-09-16 补齐首次配置与原生菜单的语言行为。新数据库根据 `NSLocale.preferredLanguages` 首项选择中文或英文，在初始化事务中保存；重开和升级保留已保存的用户选择。界面、菜单栏录音标签及应用自有菜单使用同一偏好，ASR 语言独立。

应用菜单保留 AppKit 的编辑、窗口、服务和退出动作。包内声明 `en` / `zh-Hans`，包含两份麦克风用途说明；系统服务、权限弹窗及文件选择器由 macOS 决定语言，可能与应用设置不同。[Apple 本地化声明](https://developer.apple.com/library/archive/documentation/General/Reference/InfoPlistKeyReference/Articles/CoreFoundationKeys.html)。

## 实际检查

使用独立 `com.speakmore.localecheck.20260916` 身份和空数据库，系统首选为 `zh-Hans-CN`，未更改系统语言。首次启动页面和菜单为中文；改为英文后，File/Edit/View/Window/Help 及编辑动作即时更新；通过原生 Quit 退出并重启，页面和菜单仍为英文。之后切回中文，退出重开仍保留中文。没有访问麦克风、申请系统权限、配置服务或使用实际用户数据库。

原生检查还复现了后台 WebView 中关闭动画未结束、语言列表继续遮挡页面的问题。Select 现在保留展开动画，关闭时立即卸载；保存导致触发器临时禁用时，在保存结束且用户未聚焦其他控件的情况下恢复焦点。最终构建双向切换均观察到列表关闭、页面恢复、焦点回到语言组合框。[英文截图](./screenshots/native-locale-en.jpeg)、[中文截图](./screenshots/native-locale-zh.jpeg) 已逐一检查。

核心 54 项测试通过，覆盖双语首次初始化、重开、v6 升级保留偏好及无效语言不创建数据库。桌面 82 项直接通过；另 4 项因沙箱禁止本机端口监听失败，获准运行本地合成 HTTP 服务后全部通过，合计 86 项通过、14 项按需忽略。新增桌面用例覆盖系统语言映射、菜单状态各阶段和五分钟计时边界。前端 41 项通过，TypeScript/Vite、Clippy（全部 targets、session-fixtures/target-audit/model-audio-check，-D warnings）、Rust fmt 和 plist 语法检查通过。

构建、源码摘要、原生观察和测试输出见 [结构化记录](./evidence/native-language-2026-09-16.json)。记录区分首次菜单检查、立即关闭修复、最终焦点修复三次测试构建；当前截图来自最终构建。普通应用另行重新打包，签名仍为 ad-hoc，未公证。

## 边界

原生菜单实际验证包含应用和编辑菜单标签、双向切换及退出重开；没有据此声称每个系统动作、实体键盘、菜单栏状态图标交互或录音时所有阶段均已通过。状态文字与计时由单元测试覆盖，实际录音菜单仍需物理录音检查。麦克风用途说明只验证资源打包，未弹出 TCC 对话框。英文系统首次启动有映射/存储测试，尚未在另一个英文系统环境中实测。VoiceOver、完整焦点遍历、所有浮层、200% 缩放及多屏仍待专项验收。
