import { asAppError, type UiMessage } from '@/lib/errors'
import { useCallback, useEffect, useRef, useState } from 'react'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { toast } from 'sonner'
import { call, useApp, useText } from '@/lib/app-state'
import {
  errorMessage,
  type ModelInstallState,
  type ModelsSnapshot,
  type ModelRow,
} from '@/lib/types'
import { PageTitle, InlineError, Confirm } from '@/components/common'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { ImportModelDialog } from './model-import'
import { MeetingModel } from './meeting-model'
import {
  ModelIcon,
  DownloadIcon,
  TrashIcon,
  SpinnerIcon,
  XIcon,
  ReloadIcon,
  FolderIcon,
} from '@/components/icons'

function bytes(value: number) {
  return `${(value / 1_000_000).toFixed(1)} MB`
}

export function Models({ embedded = false }: { embedded?: boolean }) {
  const t = useText(),
    { data, refresh: refreshSession } = useApp()
  const desktop = data?.desktop === true
  const [models, setModels] = useState<ModelsSnapshot | null>(null)
  const [error, setError] = useState<UiMessage | null>(null)
  const [pending, setPending] = useState(false)
  const [removing, setRemoving] = useState<ModelRow | null>(null)
  const [importing, setImporting] = useState<ModelRow | null>(null)
  const importOrigin = useRef<{ button: HTMLButtonElement; row: HTMLElement | null } | null>(null)
  const locked =
    !desktop || pending || models?.busy || !!data?.session.sessionId || !!data?.modelOperation
  const refresh = useCallback(async () => {
    const snapshot = await call<ModelsSnapshot>('list_models')
    setModels((previous) =>
      previous && previous.revision > snapshot.revision ? previous : snapshot,
    )
    setError(null)
  }, [])
  useEffect(() => {
    let disposed = false,
      off: UnlistenFn | undefined
    const update = async () => {
      try {
        const snapshot = await call<ModelsSnapshot>('list_models')
        if (!disposed) {
          setModels((p) => (p && p.revision > snapshot.revision ? p : snapshot))
          setError(null)
        }
      } catch (e) {
        if (!disposed) setError(asAppError(e))
      }
    }
    void (async () => {
      try {
        if (desktop) {
          off = await listen('models_changed', () => void update())
          if (disposed) {
            off()
            return
          }
        }
        await update()
      } catch (e) {
        if (!disposed) setError(asAppError(e))
      }
    })()
    return () => {
      disposed = true
      off?.()
    }
  }, [desktop])
  const perform = async (command: string, args: Record<string, unknown>) => {
    setPending(true)
    try {
      await call(command, args)
      await refresh()
      await refreshSession()
    } catch (e) {
      toast.error(errorMessage(e, t))
      await refresh().catch(() => {})
      await refreshSession().catch(() => {})
    } finally {
      setPending(false)
    }
  }
  const labels: Record<ModelInstallState, string> = {
    checking: t('正在检查', 'Checking'),
    absent: t('未安装', 'Not installed'),
    downloading: t('正在下载', 'Downloading'),
    importing: t('正在导入', 'Importing'),
    retrying: t('等待重试', 'Waiting to retry'),
    verifying: t('正在校验', 'Verifying'),
    installing: t('正在安装', 'Installing'),
    installed: t('已安装', 'Installed'),
    deleting: t('正在移除', 'Removing'),
    invalid: t('需要重新检查', 'Needs attention'),
    download_failed: t('下载未完成', 'Download incomplete'),
    import_failed: t('导入未完成', 'Import incomplete'),
  }
  return (
    <>
      <ImportModelDialog
        model={importing}
        close={() => setImporting(null)}
        started={() => {
          void refresh().catch((e) => setError(asAppError(e)))
          void refreshSession().catch(() => {})
        }}
        restoreFocus={() => {
          const origin = importOrigin.current
          const target = origin?.button.isConnected
            ? origin.button
            : origin?.row?.querySelector<HTMLButtonElement>('button:not(:disabled)')
          target?.focus()
        }}
      />
      <Confirm
        open={removing !== null}
        onOpenChange={(open) => {
          if (!open) setRemoving(null)
        }}
        title={t(
          `移除 ${[removing?.displayName, removing?.version].filter(Boolean).join(' · ') || '模型'}？`,
          `Remove ${[removing?.displayName, removing?.version].filter(Boolean).join(' · ') || 'model'}?`,
        )}
        description={t(
          `将释放 ${bytes(removing?.sizeBytes ?? 0)} 空间。之后使用这个模型需要重新下载。`,
          `This frees ${bytes(removing?.sizeBytes ?? 0)} of space. You can download the model again later.`,
        )}
        onConfirm={async () => {
          if (!removing) return
          await call('delete_model', { modelId: removing.modelId })
          await refresh()
        }}
      />
      {embedded ? (
        <h2 className="mb-5 text-lg font-semibold">{t('本地语音模型', 'Local speech models')}</h2>
      ) : (
        <PageTitle
          title={t('语音模型', 'Voice models')}
          description={t('选择适合这台 Mac 的声音引擎。', 'Choose a speech engine for this Mac.')}
        />
      )}
      <p className="mb-6 max-w-2xl text-sm leading-6 text-muted-foreground">
        {desktop
          ? t(
              '下载或导入后选择“使用模型”，就能离线听写。切换时会先检查文件并加载引擎；录音期间保持当前模型。',
              'Download or import, then choose Use model to dictate offline. Switching verifies the files and loads the engine. Your model stays fixed during a recording.',
            )
          : t(
              '这里是界面预览。请在 macOS 应用中下载和管理本地模型。',
              'This is an interface preview. Download and manage local models in the macOS app.',
            )}
      </p>
      {data?.modelOperation && (
        <p role="status" className="mb-5 flex items-center gap-2 text-sm text-muted-foreground">
          <SpinnerIcon className="size-4 animate-spin motion-reduce:animate-none" aria-hidden />
          {data.modelOperation === 'unloading'
            ? t('正在停用模型…', 'Unloading model…')
            : data.modelOperation === 'recovering'
              ? t('正在恢复模型…', 'Recovering model…')
              : t('正在准备模型，请稍候…', 'Preparing the model…')}
        </p>
      )}
      <InlineError message={error} />
      {error && (
        <Button
          variant="outline"
          className="my-4"
          onClick={() => void refresh().catch((e) => setError(asAppError(e)))}
        >
          <ReloadIcon />
          {t('刷新', 'Refresh')}
        </Button>
      )}
      {!models && !error && (
        <p role="status" className="py-8 text-sm text-muted-foreground">
          {t('正在读取模型列表…', 'Loading models…')}
        </p>
      )}
      {models && (
        <div className="overflow-hidden rounded-xl border bg-card">
          {models.models.map((model) => {
            const name = [model.displayName, model.version].filter(Boolean).join(' · ')
            const active = models.jobModelId === model.modelId
            const progress = Math.min(100, (model.receivedBytes / model.sizeBytes) * 100)
            const installed = model.state === 'installed'
            const selected = data?.settings.activeModelId === model.modelId
            const ready =
              selected &&
              data?.worker.phase === 'ready' &&
              data.worker.modelId === model.modelId &&
              !data.modelOperation
            const small = model.familyModelId === 'whisper-small-q5_1' && !model.version
            return (
              <section
                key={model.modelId}
                aria-label={name}
                className="border-b p-5 last:border-0 sm:p-6"
              >
                <div className="flex items-start gap-4">
                  <div className="grid size-10 shrink-0 place-items-center rounded-lg bg-muted">
                    <ModelIcon className="size-5 text-muted-foreground" aria-hidden />
                  </div>
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-2.5">
                      <h3 className="font-semibold">{model.displayName}</h3>
                      {model.version && (
                        <Badge variant="outline" className="max-w-full whitespace-normal break-all">
                          {model.version}
                        </Badge>
                      )}
                      {selected && (
                        <Badge variant="outline" className="font-normal">
                          {t('当前模型', 'Selected')}
                        </Badge>
                      )}
                      {small && (
                        <Badge
                          variant="secondary"
                          className="bg-accent font-normal text-accent-foreground"
                        >
                          {t('体积较小', 'Smaller download')}
                        </Badge>
                      )}
                    </div>
                    <p className="mt-1 text-xs text-muted-foreground">
                      {model.family === 'parakeet'
                        ? t('英语及 24 种欧洲语言', 'English and 24 other European languages')
                        : model.family === 'qwen-asr'
                          ? t('30 种语言 · 中文方言', '30 languages · Chinese dialects')
                          : model.languages.includes('multilingual')
                            ? t('多语言', 'Multilingual')
                            : model.languages
                                .map(
                                  (language) =>
                                    ({
                                      zh: t('中文', 'Chinese'),
                                      yue: t('粤语', 'Cantonese'),
                                      en: t('英语', 'English'),
                                      ja: t('日语', 'Japanese'),
                                      ko: t('韩语', 'Korean'),
                                    })[language] ?? language,
                                )
                                .join(' / ')}
                      <span className="mx-2">·</span>
                      {model.quantization.toUpperCase()}
                      <span className="mx-2">·</span>
                      {bytes(model.sizeBytes)}
                      <span className="mx-2">·</span>
                      {t('含语音检测组件', 'Includes speech detection')}
                    </p>
                    <p className="mt-3 text-sm leading-6 text-muted-foreground">
                      {model.family === 'parakeet'
                        ? t(
                            '适合英文与欧洲语言听写、录音转写。自动识别语言，不支持中文。',
                            'For English and European-language dictation and recordings. Detects language automatically; Chinese is not supported.',
                          )
                        : model.family === 'qwen-asr'
                          ? t(
                              '多语言与中文方言本地识别。自动识别语言，与用于整理和翻译的 Qwen3 文字模型分别下载。',
                              'Local multilingual and Chinese dialect recognition with automatic language detection. Downloaded separately from the Qwen3 text model for summaries and translation.',
                            )
                          : model.family === 'sensevoice'
                            ? t(
                                '支持五种语言的本地转写。长录音需要较多内存，转写结果请先检查。',
                                'Local transcription in five languages. Long recordings use more memory; review the transcript before use.',
                              )
                            : small
                              ? t(
                                  '较低的资源占用，适合日常消息与写作。',
                                  'Lower resource usage for everyday messages and writing.',
                                )
                              : t(
                                  '面向更复杂的多语言表达，需要更多运行资源。',
                                  'For more demanding multilingual speech, with higher resource requirements.',
                                )}
                    </p>
                    <p className="mt-2 text-xs leading-5 text-muted-foreground">
                      {model.dictionaryPrompt
                        ? t('支持个人词典提示', 'Supports personal dictionary prompts')
                        : t(
                            '此模型暂不使用个人词典',
                            'This model does not use your personal dictionary',
                          )}
                    </p>
                    <p className="mt-1 text-xs leading-5 text-muted-foreground">
                      {t('模型许可：', 'Model license: ')}
                      {model.licenseLabel}
                    </p>
                    <div className="mt-4 flex flex-wrap items-center justify-between gap-3">
                      <span
                        className="flex items-center gap-2 text-xs text-muted-foreground"
                        role="status"
                      >
                        {active && (
                          <SpinnerIcon
                            className="size-3.5 animate-spin motion-reduce:animate-none"
                            aria-hidden
                          />
                        )}
                        {ready
                          ? `${t('已就绪', 'Ready')} · ${data.worker.backend === 'metal' ? 'Metal' : data.worker.backend.toUpperCase()}`
                          : labels[model.state]}
                        {model.state === 'retrying' && ` · ${model.attempt}/3`}
                        {!model.supported &&
                          ` · ${t('此设备暂不支持', 'Unavailable on this device')}`}
                      </span>
                      <div className="flex flex-wrap gap-2">
                        {active ? (
                          <Button
                            size="sm"
                            variant="outline"
                            disabled={pending || !models.cancellable}
                            aria-label={
                              models.jobKind === 'import'
                                ? t(`取消导入 ${name}`, `Cancel import of ${name}`)
                                : t(`取消下载 ${name}`, `Cancel download of ${name}`)
                            }
                            onClick={() =>
                              void perform('cancel_model_download', { jobId: models.activeJobId })
                            }
                          >
                            <XIcon aria-hidden />
                            {models.cancellable
                              ? models.jobKind === 'import'
                                ? t('取消导入', 'Cancel import')
                                : t('取消下载', 'Cancel download')
                              : t('处理中', 'Finishing')}
                          </Button>
                        ) : installed ? (
                          <>
                            <Button
                              size="sm"
                              variant={selected ? 'outline' : 'default'}
                              disabled={locked}
                              aria-label={
                                selected
                                  ? t(`停用 ${name}`, `Unload ${name}`)
                                  : t(`使用模型 ${name}`, `Use model ${name}`)
                              }
                              onClick={() =>
                                void perform('select_model', {
                                  modelId: selected ? null : model.modelId,
                                  expectedRevision: data?.settings.revision,
                                })
                              }
                            >
                              {selected ? t('停用', 'Unload') : t('使用模型', 'Use model')}
                            </Button>
                            {selected && (
                              <Button
                                size="sm"
                                variant="outline"
                                disabled={locked}
                                aria-label={t(`重新加载 ${name}`, `Reload ${name}`)}
                                onClick={() => void perform('reload_model', {})}
                              >
                                <ReloadIcon aria-hidden />
                                {t('重新加载', 'Reload')}
                              </Button>
                            )}
                            <Button
                              size="sm"
                              variant="ghost"
                              disabled={locked || selected}
                              aria-label={t(`移除 ${name}`, `Remove ${name}`)}
                              onClick={() => setRemoving(model)}
                            >
                              <TrashIcon aria-hidden />
                              {t('移除', 'Remove')}
                            </Button>
                          </>
                        ) : (
                          <>
                            <Button
                              size="sm"
                              variant="outline"
                              disabled={locked || !model.supported || model.state === 'checking'}
                              onClick={() =>
                                void perform('download_model', { modelId: model.modelId })
                              }
                              aria-label={t(`下载 ${name}`, `Download ${name}`)}
                            >
                              <DownloadIcon aria-hidden />
                              {model.error
                                ? t('重新下载', 'Retry download')
                                : t('下载模型', 'Download')}
                            </Button>
                            <Button
                              size="sm"
                              variant="ghost"
                              disabled={locked || !model.supported || model.state === 'checking'}
                              aria-label={t(`导入 ${name}`, `Import ${name}`)}
                              onClick={(event) => {
                                importOrigin.current = {
                                  button: event.currentTarget,
                                  row: event.currentTarget.closest('section'),
                                }
                                setImporting(model)
                              }}
                            >
                              <FolderIcon aria-hidden />
                              {t('从本机导入', 'Import local files')}
                            </Button>
                          </>
                        )}
                      </div>
                    </div>
                    {active && (
                      <div className="mt-4">
                        <div
                          role="progressbar"
                          aria-label={
                            models.jobKind === 'import'
                              ? t('模型导入进度', 'Model import progress')
                              : t('模型下载进度', 'Model download progress')
                          }
                          aria-valuemin={0}
                          aria-valuemax={100}
                          aria-valuenow={Math.round(progress)}
                          className="h-1 overflow-hidden rounded-full bg-muted"
                        >
                          <div
                            className="h-full bg-accent-foreground transition-[width] duration-200 motion-reduce:transition-none"
                            style={{ width: `${progress}%` }}
                          />
                        </div>
                        <p className="mt-2 text-right text-xs tabular-nums text-muted-foreground">
                          {bytes(model.receivedBytes)} / {bytes(model.sizeBytes)}
                        </p>
                      </div>
                    )}
                    {model.error && (
                      <p role="alert" className="mt-3 text-sm leading-6 text-destructive">
                        {errorMessage({ code: model.error, userMessage: model.errorMessage }, t)}
                      </p>
                    )}
                  </div>
                </div>
              </section>
            )
          })}
        </div>
      )}
      <MeetingModel />
    </>
  )
}
