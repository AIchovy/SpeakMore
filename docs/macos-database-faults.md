# 数据库真实磁盘满与提交后崩溃检查

2026-09-16，使用真实 `Store::persist_generated`、`Store::open` 和原始迁移脚本，在独立的 512 MiB APFS 镜像中完成三项检查。全部使用合成数据库；不打开用户数据库、模型、麦克风或凭据库。诊断入口仅编译进显式运行的 `cfg(test)` 测试程序，普通应用不包含该入口。

| 场景 | 故障与结果 | 恢复验证 |
| --- | --- | --- |
| 历史写入磁盘满 | Store 已打开后实际填充镜像，1 MiB 与 4 KiB 写入均收到 ENOSPC；保存 16,384 bytes 的合成结果返回 StorageUnavailable | 新历史与对应 usage 均不存在，原表摘要不变；释放填充文件后同一 session 成功保存，再次提交不重复计数或插入 |
| 提交后 SIGKILL | `persist_generated` 成功返回后、数据库仍打开时终止所属测试子进程 | 保留了 45,352 bytes 的未 checkpoint WAL；正常 SQLite 重开后，原结果与新结果各一条，usage 同步；再次提交同一 session 无重复 |
| 升级备份空间不足 | 真实 migrations 001–006 创建的 v6 数据库含 1,024 条合成历史，主库与旧备份各 17,137,664 bytes；填充镜像至 ENOSPC 后释放独立 2 MiB 预留文件，升级返回 DatabaseBackupFailed | 原库仍为 v6，所有旧字段摘要与旧备份 SHA-256 不变；释放填充文件后正常升级至 v7，新增 polish_reason 均为空，旧数据保持一致，已确认健康后才清理已知备份；未知恢复说明文件保留 |

证据见 [结构化记录](./evidence/macos-database-faults-2026-09-16.json) 与 [完整原始输出](./evidence/macos-database-faults-2026-09-16.raw.json.gz)。固定测试程序 SHA-256 为 `f876379e8b9872d13adf399e4c95773fab5fe077f59031e7b3a516f84583ec14`。每次运行先复制测试可执行文件，避免编译器替换后续用例的程序。测试盘已卸载。

核对方式与边界：

- 每个子进程退出后才通过 SQLite 重开；保留 WAL 并让 SQLite 正常恢复，不采用忽略日志的 immutable 读法。逐表记录列名、行数和内容摘要，同时执行 integrity_check、foreign_key_check。升级比较全部旧列，并单独确认新增列为空。
- 旧备份通过 SQLite Backup API 创建。失败后按原文件摘要确认未被替换；成功升级后检查已知临时文件和备份被清理，未知文件保留。备份 API 的快照与错误处理定义见 [SQLite 官方说明](https://www.sqlite.org/backup.html)。
- 合成数据包含历史、使用记录、设置、词典、应用规则和旧许可行。该组测试中的模型、服务、快捷键和待清理凭据表为空，不能据此声称验证了真实模型或服务凭据的恢复；非空模型安装恢复另见 [模型文件系统检查](./macos-filesystem-faults.md)。
- 本次实际发生 ENOSPC 时，statvfs 仍报告约 11 MiB 可用；该数值不能替代真正写入成功的证据。检查器既记录空间统计，也要求实际分配失败及应用错误，不伪造 SQLite 错误返回。
- 首次备份场景的检查器在 APFS 满盘后缩小填充文件也收到 ENOSPC。改为预先分配独立保留文件、需要时删除，随后用固定程序重新跑完三项。此为诊断工具调整，没有修改产品的事务或备份逻辑。
- SIGKILL 的检查点位于成功提交后，不在事务内部；满盘升级只覆盖备份阶段，未覆盖成功备份后的迁移提交。物理断电、磁盘损坏、SQLite 事务内部终止、安装器升级仍需另行验收。SQLite 对原子提交与崩溃恢复有独立机制和假设，不能把进程终止等同于物理掉电；参见 [SQLite 原子提交说明](https://www.sqlite.org/atomiccommit.html)。

复现使用 `scripts/diagnostics/macos-database-faults.py`，显式传入桌面测试程序、全新输出目录和已挂载的限容测试盘；目录必须位于 `.cache/fs-fault-study`。脚本核验独立设备号、容量、非符号链接路径及专属标记，只操作自己创建的填充/保留文件。运行期间不要手动卸载测试镜像，结束后需卸载。Rust fmt、Python 编译检查及 desktop all-targets Clippy（session-fixtures,target-audit，-D warnings）已通过。

最终工具审查另补充了保留文件的所有权保护：若同名文件已经存在，必须返回 EEXIST 并保持原文件，不能在异常清理中删除它。该失败分支的独立检查通过；成功路径与 Rust 诊断动作没有变化。证据分别保存三项故障运行时的源码摘要和此次工具保护变更后的摘要。
