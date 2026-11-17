import { asAppError, type UiMessage } from '@/lib/errors'
import { useEffect, useState } from 'react'
import { call } from '@/lib/app-state'
import { errorMessage, type AppError } from '@/lib/types'
import { Button } from '@/components/ui/button'
import { FolderIcon, ReloadIcon, ShieldIcon, WarningIcon } from '@/components/icons'

export interface StorageStatus {
  error: AppError | null
  cleanupPending: boolean
  canReveal: boolean
}

export function useStorageStatus() {
  const [status, setStatus] = useState<StorageStatus | null>(null)
  useEffect(() => {
    let active = true
    void call<StorageStatus>('get_storage_status')
      .then((next) => {
        if (active) setStatus(next)
      })
      .catch(() => {
        /* Generic startup error remains visible if this endpoint fails. */
      })
    return () => {
      active = false
    }
  }, [])
  return status
}

export function recoverySteps(code: string, english = false): string[] {
  if (english) {
    switch (code) {
      case 'NewerDatabase':
        return [
          'Install the same or a newer SpeakMore version than the one that created this database.',
          'Do not clear the data folder or use an older app to bypass the version check.',
        ]
      case 'DatabaseBackupFailed':
        return [
          'Check available disk space and read/write access to the SpeakMore data folder.',
          'Once the problem is resolved, reopen the app to retry the backup and upgrade.',
        ]
      case 'DatabaseMigrationFailed':
        return [
          'Quit the app and keep the data folder, including its recovery folder.',
          'If reopening fails again, retry with a fixed app version. Save a separate copy of the current data folder before restoring a backup.',
        ]
      case 'DatabaseCorrupt':
      case 'UnknownDatabase':
      case 'DatabaseRecoveryRequired':
        return [
          'Quit the app and save a separate copy of the entire data folder, including any SQLite journal files.',
          'Restore only a verified backup. If none is available, keep the original files for diagnosis.',
        ]
      default:
        return [
          'Check available disk space and access to the data folder.',
          'Reopen the app after resolving the problem. Keep the existing files and do not clear the data folder.',
        ]
    }
  }
  switch (code) {
    case 'NewerDatabase':
      return [
        '请安装创建这份数据的同版或更新版本 SpeakMore。',
        '不要用旧版应用覆盖数据，也不要删除数据文件来绕过版本检查。',
      ]
    case 'DatabaseBackupFailed':
      return [
        '检查磁盘可用空间，以及 SpeakMore 数据目录的读写权限。',
        '问题修复后重新打开应用，再次尝试备份和升级。',
      ]
    case 'DatabaseMigrationFailed':
      return [
        '先退出应用，保留数据目录和其中的恢复文件夹。',
        '重新打开仍失败时，请使用修复后的应用版本重试；恢复旧备份前，先另存现有数据目录。',
      ]
    case 'DatabaseCorrupt':
    case 'UnknownDatabase':
    case 'DatabaseRecoveryRequired':
      return [
        '先退出应用，另存整个数据目录，包括可能存在的日志文件。',
        '使用经过校验的备份恢复；没有可用备份时，请保留原文件用于诊断。',
      ]
    default:
      return [
        '检查磁盘可用空间和数据目录权限。',
        '排除问题后重新打开应用；请保留现有文件，不要清空数据目录。',
      ]
  }
}

function RevealButton({ disabled, english = false }: { disabled: boolean; english?: boolean }) {
  const t = (zh: string, en: string) => (english ? en : zh)
  const [busy, setBusy] = useState(false),
    [error, setError] = useState<UiMessage | null>(null)
  return (
    <div>
      <Button
        variant="outline"
        disabled={disabled || busy}
        onClick={() => {
          setBusy(true)
          setError(null)
          void call('reveal_recovery_data')
            .catch((e) => setError(asAppError(e)))
            .finally(() => setBusy(false))
        }}
      >
        <FolderIcon />
        {english ? 'Show data files' : '查看数据文件'}
      </Button>
      {error && (
        <p role="alert" className="mt-2 text-sm text-destructive">
          {typeof error === 'string' ? error : errorMessage(error, t)}
        </p>
      )}
    </div>
  )
}

export function StorageRecovery({
  status,
  initialEnglish = false,
}: {
  status: StorageStatus
  initialEnglish?: boolean
}) {
  const [english, setEnglish] = useState(initialEnglish)
  const t = (zh: string, en: string) => (english ? en : zh)
  const [busy, setBusy] = useState(false),
    [error, setError] = useState<UiMessage | null>(null)
  const problem = status.error
  if (!problem) return null
  return (
    <main className="grid min-h-screen place-items-center bg-background px-6 py-10">
      <section
        lang={english ? 'en' : 'zh-CN'}
        aria-labelledby="recovery-title"
        className="w-full max-w-lg"
      >
        <div className="mb-6 flex items-center gap-3 text-sm font-medium text-muted-foreground">
          <ShieldIcon className="size-5" />
          SpeakMore
          <Button
            variant="ghost"
            size="sm"
            className="ml-auto"
            onClick={() => setEnglish(!english)}
            aria-label={t('切换为英文', 'Switch to Chinese')}
          >
            {english ? '中文' : 'English'}
          </Button>
        </div>
        <h1 id="recovery-title" className="text-2xl font-semibold tracking-tight">
          {t('本地数据需要处理', 'Your local data needs attention')}
        </h1>
        <p role="alert" className="mt-4 text-sm leading-7 text-foreground">
          {english
            ? problem.code === 'NewerDatabase'
              ? 'This data was created by a newer version of SpeakMore.'
              : problem.code === 'DatabaseBackupFailed'
                ? 'The upgrade backup could not be completed. Data migration has stopped.'
                : problem.code === 'DatabaseMigrationFailed'
                  ? 'The data upgrade did not complete. Original files have been kept and dictation has not started.'
                  : problem.code === 'DatabaseRecoveryRequired'
                    ? 'The main database is missing or empty, but recovery files still exist. Initialization has stopped.'
                    : 'SpeakMore could not safely open local data. Keep the original files and follow the recovery steps.'
            : problem.userMessage}
        </p>
        <ol className="my-6 list-decimal space-y-3 rounded-xl border bg-card px-8 py-5 text-sm leading-7">
          {recoverySteps(problem.code, english).map((step) => (
            <li key={step}>{step}</li>
          ))}
        </ol>
        <p className="mb-6 text-xs leading-6 text-muted-foreground">
          {t(
            '升级前备份位于数据目录的 speakmore.sqlite3.recovery 文件夹；成功升级并检查后会清理。备份可能包含已保存的历史和设置，仅保存在本机，不会自动上传。',
            'Upgrade backups are kept in speakmore.sqlite3.recovery inside the data folder and removed after a successful, verified upgrade. Backups may contain saved history and settings. They stay on this Mac and are not uploaded automatically.',
          )}
        </p>
        <div className="flex flex-wrap items-start gap-3">
          <RevealButton disabled={!status.canReveal} english={english} />
          <Button
            disabled={busy}
            onClick={() => {
              setBusy(true)
              setError(null)
              void call('restart_after_storage_error').catch((e) => {
                setError(asAppError(e))
                setBusy(false)
              })
            }}
          >
            <ReloadIcon />
            {busy ? t('正在重新打开…', 'Reopening…') : t('重新打开应用', 'Reopen app')}
          </Button>
        </div>
        {error && (
          <p role="alert" className="mt-3 text-sm text-destructive">
            {typeof error === 'string' ? error : errorMessage(error, t)}
          </p>
        )}
        <p className="mt-6 text-xs text-muted-foreground">
          {t('错误标识：', 'Error code: ')}
          {problem.code}
        </p>
      </section>
    </main>
  )
}

export function StorageBackupNotice({
  status,
  english = false,
}: {
  status: StorageStatus
  english?: boolean
}) {
  const t = (zh: string, en: string) => (english ? en : zh)
  if (!status.cleanupPending) return null
  return (
    <aside
      className="mx-6 mt-5 flex flex-wrap items-center gap-4 rounded-lg border bg-muted/40 p-4 md:mx-8"
      aria-label={t('升级备份保留提示', 'Upgrade backup retained')}
    >
      <WarningIcon className="size-5 shrink-0" />
      <div className="min-w-56 flex-1 text-sm leading-6">
        <p className="font-medium">
          {t('升级备份尚未清理', 'An upgrade backup is still on this Mac')}
        </p>
        <p className="text-muted-foreground">
          {t(
            '应用可以继续使用。备份可能包含历史记录；请检查数据目录权限，下次启动会重试清理。',
            'You can continue using the app. The backup may contain history. Check data folder permissions; cleanup will be retried on the next launch.',
          )}
        </p>
      </div>
      <RevealButton disabled={!status.canReveal} english={english} />
    </aside>
  )
}
