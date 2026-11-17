# macOS 模型安装中断与磁盘写满检查

2026-09-16，在隔离合成数据目录验证了真实 `ModelManager → SqliteModels → Store` 链路。五项通过，未修改用户数据库或生产模型。测试专用入口由 `cfg(test)` 编译，使用公开测试签名钥与合成模型字节，不属于产品 IPC，也不声称模型可用于识别。

| 场景 | 实际操作 | 重启后的结果 |
| --- | --- | --- |
| 复制中断 | 第一次复制进度后向受控子进程发送 SIGKILL | 本次 staging 清理，新版本未安装；显式重试成功 |
| 校验完成、改名前 | Installing 通知处 SIGKILL | 本次 staging 清理，新版本未安装；显式重试成功 |
| 已改名、SQLite 登记前 | Registry 调用前 SIGKILL | 已验证文件保留；重启复核后登记成功 |
| SQLite 登记后 | 实际登记已提交、发布 Installed 前 SIGKILL | 重启保持一致的安装状态 |
| 复制期间 ENOSPC | 空间预检通过后，在 512 MiB APFS 镜像中实际写到 errno 28，再继续复制 | 返回 ImportFailed / InsufficientSpace；释放测试填充文件后可重启并重试成功 |

所有场景均核对旧模型文件与当前选择、导入源文件、历史、词典、设置、使用统计保持不变；SQLite integrity / foreign-key 检查通过。专有 staging 的清理没有删除不属于任务的哨兵文件。

最终记录见 [结构化证据](./evidence/macos-filesystem-faults-2026-09-16.json) 与 [完整原始输出](./evidence/macos-filesystem-faults-2026-09-16.raw.json.gz)。测试程序在启动前复制为独立、固定的可执行文件，避免并行构建改变后续用例所运行的版本。最终固定程序 SHA-256 为 `94e189e931850ed2e99025e6fecea8f425ddc084870d7073efebe46dc70fa089`。

测试盘容量 536,829,952 bytes；最后一次向专用填充文件写入 387,973,120 bytes 后实际收到 ENOSPC。填充文件随后删除，测试镜像已卸载。填充器核对设备号与父目录不同、容量不超过 600 MiB、路径属于专用 cache 且无符号链接，并限制总写入不超过 512 MiB。不会填满宿主磁盘。先前调试发现检查器的错误码大小写不一致及只读 SQLite 打开方式不适用于本机 WAL；修正检查器后固定程序重新跑完五项。没有因此修改安装器的产品逻辑。

复现入口为 `scripts/diagnostics/macos-filesystem-faults.py`，要求显式传入桌面测试程序、全新 `--run-directory` 和已经挂载的限容 `--volume`。用例仅在 `.cache/fs-fault-study` 下的标记目录执行。每个子进程退出后再通过 SQLite 正常打开做数据核对；不使用 immutable 读取绕过 WAL，也不复制正在使用中的裸数据库。这遵循 [SQLite 数据库损坏说明](https://www.sqlite.org/howtocorrupt.html) 中保留恢复日志、正确处理活动数据库的要求。

这组证据只覆盖安装边界的进程终止和模型复制时的真实空间不足。未覆盖物理断电/磁盘损坏、SQLite 事务内部被终止、数据库备份或历史写入时的真实 ENOSPC、安装器升级、用户目录权限损坏以及实际识别。因此 M01/M02 与 U01 的其余发布条件仍需独立验收，不能由本测试统一放行。

同日随后完成的数据库写入磁盘满、提交后 SIGKILL 与升级备份空间不足检查见 [数据库故障验证](./macos-database-faults.md)，属于独立测试与证据，不扩大本页模型测试本身的覆盖范围。
