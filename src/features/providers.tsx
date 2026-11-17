import { useCallback, useEffect, useRef, useState } from 'react'
import { isTauri } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { call, useApp, useText } from '@/lib/app-state'
import { providerErrorMessage } from './provider-errors'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Badge } from '@/components/ui/badge'
import { Choice, Confirm, EmptyState, InlineError } from '@/components/common'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  CheckIcon,
  EditIcon,
  ModelIcon,
  PlusIcon,
  ReloadIcon,
  ShieldIcon,
  SpinnerIcon,
  TrashIcon,
} from '@/components/icons'
import { CloudPolicy } from './cloud-policy'
import { ProviderRecoveryRows } from './provider-recovery'
import {
  consumeCredential,
  endpoints,
  localOllama,
  providerNames,
  type CredentialIntent,
  type ProviderProtocol,
  type ProviderRow,
  type ProvidersView,
  type ProviderRecoveryRow,
} from './providers-types'

type Draft = {
  expectedRevision: number
  original: ProviderRow | null
  displayName: string
  protocol: ProviderProtocol
  endpoint: string
  model: string
  credentialIntent: CredentialIntent
}

export function ProviderRows({
  rows,
  activeId,
  disabled,
  edit,
  remove,
  test,
  select,
}: {
  rows: ProviderRow[]
  activeId: string | null
  disabled: boolean
  edit: (row: ProviderRow, trigger: HTMLButtonElement) => void
  remove: (row: ProviderRow, trigger: HTMLButtonElement) => void
  test: (row: ProviderRow) => void
  select: (row: ProviderRow) => void
}) {
  const t = useText()
  return (
    <div className="divide-y overflow-hidden rounded-xl border bg-card">
      {rows.map((row) => (
        <div className="flex flex-wrap gap-4 p-5" key={row.providerId}>
          <ModelIcon className="mt-1 size-5 shrink-0 text-muted-foreground" aria-hidden />
          <div className="min-w-0 flex-1 basis-60">
            <div className="flex flex-wrap items-center gap-2">
              <h3 className="break-words font-medium">{row.displayName}</h3>
              {row.providerId === activeId && (
                <Badge variant="secondary">{t('已选择', 'Selected')}</Badge>
              )}
              <span className="text-xs text-muted-foreground">
                {row.local ? t('本机服务', 'Local service') : t('云端服务', 'Cloud service')}
              </span>
            </div>
            <p className="mt-1 break-all text-sm text-muted-foreground">{row.model}</p>
            <p className="mt-1 break-all text-xs leading-5 text-muted-foreground">{row.endpoint}</p>
            <div className="mt-3 flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
              <span>
                {row.preparedThisRun
                  ? t('本次运行已准备', 'Prepared for this run')
                  : row.testValid
                    ? t('测试记录有效，使用时准备', 'Test record valid; prepares on use')
                    : row.testedAt !== null
                      ? t('重新测试后可使用', 'Retest before use')
                      : t('尚未测试', 'Not tested')}
              </span>
              {row.testedAt !== null && Number.isFinite(new Date(row.testedAt).getTime()) && (
                <span>
                  {t('上次通过：', 'Last passed: ')}
                  <time dateTime={new Date(row.testedAt).toISOString()}>
                    {new Date(row.testedAt).toLocaleString(t('zh-CN', 'en'))}
                  </time>
                </span>
              )}
              {!row.local && (
                <>
                  <span>
                    {row.credentialConfigured
                      ? t('密钥已保存', 'Key saved')
                      : t('未保存密钥', 'No saved key')}
                  </span>
                  <span>
                    {row.destinationConfirmed
                      ? t('发送地址已确认', 'Destination confirmed')
                      : t('发送地址待确认', 'Destination not confirmed')}
                  </span>
                </>
              )}
            </div>
          </div>
          <div className="flex flex-wrap items-center gap-1 self-center">
            <Button variant="outline" size="sm" disabled={disabled} onClick={() => test(row)}>
              {t('测试连接', 'Test connection')}
            </Button>
            {row.providerId !== activeId && (
              <Button
                variant="ghost"
                size="sm"
                disabled={disabled || !row.testValid}
                onClick={() => select(row)}
              >
                <CheckIcon aria-hidden />
                {t('使用', 'Use')}
              </Button>
            )}
            <Button
              variant="ghost"
              size="icon"
              disabled={disabled}
              aria-label={t(`编辑 ${row.displayName}`, `Edit ${row.displayName}`)}
              onClick={(e) => edit(row, e.currentTarget)}
            >
              <EditIcon aria-hidden />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              disabled={disabled}
              aria-label={t(`删除 ${row.displayName}`, `Delete ${row.displayName}`)}
              onClick={(e) => remove(row, e.currentTarget)}
            >
              <TrashIcon aria-hidden />
            </Button>
          </div>
        </div>
      ))}
    </div>
  )
}

export function Providers() {
  const { data, refresh } = useApp(),
    t = useText()
  const [view, setView] = useState<ProvidersView | null>(null),
    [busy, setBusy] = useState(false)
  const [error, setError] = useState<unknown>(null),
    [draftError, setDraftError] = useState<unknown>(null)
  // The expected revision is read when confirming: it is the settings revision,
  // which other settings changes advance while a dialog is open.
  const [draft, setDraft] = useState<Draft | null>(null),
    [removing, setRemoving] = useState<{ row: ProviderRow } | null>(null)
  const [recovering, setRecovering] = useState<{ row: ProviderRecoveryRow } | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const keyField = useRef<HTMLInputElement>(null),
    addButton = useRef<HTMLButtonElement>(null),
    trigger = useRef<HTMLElement | null>(null)
  const live = useRef(true),
    fetchId = useRef(0),
    working = useRef(false)
  const reload = useCallback(async () => {
    const id = ++fetchId.current
    const next = await call<ProvidersView>('get_providers')
    if (live.current && id === fetchId.current) setView(next)
  }, [])
  useEffect(() => {
    live.current = true
    let disposed = false
    let off: (() => void) | undefined
    if (isTauri())
      void listen('providers_changed', () => {
        if (disposed) return
        void reload().catch((e) => {
          if (live.current) setError(e)
        })
      })
        .then((unlisten) => {
          if (!disposed) off = unlisten
          else unlisten()
        })
        .catch((e) => {
          if (!disposed) setError(e)
        })
    return () => {
      disposed = true
      live.current = false
      ++fetchId.current
      off?.()
      if (keyField.current) keyField.current.value = ''
    }
  }, [reload])
  useEffect(() => {
    void reload().catch((e) => {
      if (live.current) setError(e)
    })
  }, [reload, data?.settings.revision])
  useEffect(() => {
    if (!view?.busy) return
    const timer = setInterval(() => {
      void reload().catch(() => {})
    }, 1000)
    return () => clearInterval(timer)
  }, [view?.busy, reload])
  const disabled = busy || !!view?.busy || !!data?.session.sessionId
  const restoreFocus = () =>
    (trigger.current?.isConnected ? trigger.current : addButton.current)?.focus()
  const openDraft = (row: ProviderRow | null, element?: HTMLElement) => {
    if (!view) return
    trigger.current = element ?? addButton.current
    setDraftError(null)
    setNotice(null)
    setDraft({
      expectedRevision: view.revision,
      original: row,
      displayName: row?.displayName ?? 'OpenAI',
      protocol: row?.protocol ?? { kind: 'openai' },
      endpoint: row?.endpoint ?? endpoints.openai,
      model: row?.model ?? '',
      credentialIntent: row?.credentialConfigured ? 'keep' : 'replace',
    })
  }
  async function action(command: string, args: Record<string, unknown>, message?: string) {
    if (working.current) return
    working.current = true
    setBusy(true)
    setError(null)
    setNotice(null)
    try {
      try {
        await call(command, args)
      } catch (e) {
        if (live.current) setError(e)
        throw e
      }
      // The command succeeded: show a failed reload without reporting the
      // command as failed, which would invite repeating it.
      await reload()
        .then(() => refresh())
        .catch((e) => {
          if (live.current) setError(e)
        })
      if (live.current && message) setNotice(message)
    } finally {
      working.current = false
      if (live.current) setBusy(false)
    }
  }
  async function save() {
    if (!draft || working.current) return
    const local = localOllama(draft.protocol.kind, draft.endpoint)
    // Consume and clear the DOM password field before starting asynchronous IPC.
    const credential = consumeCredential(
      local ? 'remove' : draft.credentialIntent,
      keyField.current,
    )
    working.current = true
    setBusy(true)
    setDraftError(null)
    try {
      await call('save_provider', {
        input: {
          expectedRevision: draft.expectedRevision,
          providerId: draft.original?.providerId ?? null,
          providerRevision: draft.original?.revision ?? null,
          displayName: draft.displayName.trim(),
          configuration: {
            protocol: draft.protocol,
            endpoint: draft.endpoint.trim(),
            model: draft.model.trim(),
          },
          credential,
          // Saving the configured service confirms its destination without a separate checkbox.
          confirmDestination: true,
        },
      })
      await reload()
      await refresh()
      if (live.current) {
        setDraft(null)
        setNotice(
          t('服务已保存，可以测试连接。', 'Service saved. You can now test the connection.'),
        )
      }
    } catch (e) {
      if (live.current) {
        setDraftError(e)
        await reload().catch(() => {})
      }
    } finally {
      working.current = false
      if (live.current) setBusy(false)
    }
  }
  const local = draft ? localOllama(draft.protocol.kind, draft.endpoint) : false
  const hasExistingKey = !!draft?.original?.credentialConfigured
  return (
    <div className="space-y-6">
      <p className="max-w-2xl text-sm leading-6 text-muted-foreground">
        {t(
          '保存并测试你自己的 AI 服务。选择后，可在写作模式中单独开启文字整理。',
          'Save and test your own AI services. After selecting one, enable text refinement in Writing modes.',
        )}
      </p>
      <CloudPolicy />
      <p className="text-sm leading-6 text-muted-foreground">
        {t(
          '当前 AI 服务可用于听写整理与写作格式、会议纪要、字幕翻译。各功能复用地址和密钥，可在对应页面单独选择。',
          'The current AI service is shared by dictation and writing styles, meeting summaries, and subtitle translation. Each feature can override the selection.',
        )}
      </p>
      <div className="flex items-center justify-between gap-3">
        <h2 className="text-base font-semibold">{t('我的服务', 'My services')}</h2>
        <div className="flex items-center gap-2">
          <Button
            variant="ghost"
            size="icon"
            disabled={disabled}
            aria-label={t('刷新服务列表', 'Refresh service list')}
            onClick={() =>
              void reload()
                .then(() => setError(null))
                .catch(setError)
            }
          >
            <ReloadIcon aria-hidden />
          </Button>
          <Button
            ref={addButton}
            variant="outline"
            size="sm"
            disabled={disabled || !view || view.totalProviders >= 20}
            onClick={() => openDraft(null)}
          >
            <PlusIcon aria-hidden />
            {t('添加服务', 'Add service')}
          </Button>
        </div>
      </div>
      <InlineError message={providerErrorMessage(error, t)} />
      {view?.truncated && (
        <p role="status" className="text-sm text-muted-foreground">
          {t(
            '本地配置数量异常，当前最多显示每类 100 条。处理后请刷新列表；添加服务仍受 20 条上限约束。',
            'Local settings exceed the usual limit. Up to 100 records per category are shown. Refresh after resolving entries; the 20-service limit still applies.',
          )}
        </p>
      )}
      {view && (
        <ProviderRecoveryRows
          rows={view.recoveryRows}
          disabled={disabled}
          review={(row, element) => {
            trigger.current = element
            setRecovering({ row })
          }}
        />
      )}
      {notice && (
        <p role="status" className="text-sm text-accent-foreground">
          {notice}
        </p>
      )}
      {view?.busy && (
        <div
          role="status"
          className="flex flex-wrap items-center gap-3 rounded-lg bg-muted p-3 text-sm"
        >
          <SpinnerIcon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
          <span>
            {view.operation === 'testing'
              ? t('正在测试连接…', 'Testing connection…')
              : view.operation === 'saving_test'
                ? t('正在保存测试结果…', 'Saving test result…')
                : t(
                    '正在处理服务配置或系统钥匙串，请稍候…',
                    'Processing service settings or the system keychain…',
                  )}
          </span>
          {view.operation === 'testing' && (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => void call('cancel_provider_test').catch((e) => setError(e))}
            >
              {t('取消测试', 'Cancel test')}
            </Button>
          )}
        </div>
      )}
      {!!view?.cleanupPending && (
        <div className="space-y-3 rounded-lg border p-4">
          <div className="flex gap-2 text-sm">
            <ShieldIcon aria-hidden className="mt-0.5 size-4 shrink-0" />
            <p>
              {t(
                '还有密钥等待从系统钥匙串清理，尚未确认完全删除。',
                'Some keys are still awaiting removal from the system keychain. Cleanup has not been confirmed.',
              )}
            </p>
          </div>
          <InlineError message={providerErrorMessage(view.cleanupError, t)} />
          <Button
            variant="outline"
            size="sm"
            disabled={disabled}
            onClick={() => void action('retry_provider_cleanup', {}).catch(() => {})}
          >
            {t('重试清理', 'Retry cleanup')}
          </Button>
        </div>
      )}
      {view ? (
        view.rows.length ? (
          <ProviderRows
            rows={view.rows}
            activeId={view.activeProviderId}
            disabled={disabled}
            edit={(row, element) => openDraft(row, element)}
            remove={(row, element) => {
              trigger.current = element
              setRemoving({ row })
            }}
            test={(row) =>
              void action(
                'test_provider',
                { providerId: row.providerId, revision: row.revision },
                t('连接测试通过。', 'Connection test passed.'),
              ).catch(() => {})
            }
            select={(row) =>
              void action(
                'select_provider',
                { expectedRevision: view.revision, providerId: row.providerId },
                t(
                  '已选择此服务；文字整理仍需单独开启。',
                  'Service selected. Text polishing still needs to be enabled separately.',
                ),
              ).catch(() => {})
            }
          />
        ) : (
          <EmptyState
            icon={ModelIcon}
            title={t('连接你熟悉的 AI 服务', 'Connect an AI service you use')}
            description={t(
              '支持 OpenAI、Anthropic、Ollama 和兼容接口。本地 Ollama 不需要 API Key。',
              'Supports OpenAI, Anthropic, Ollama and compatible endpoints. Local Ollama needs no API key.',
            )}
            action={
              <Button
                disabled={disabled || view.totalProviders >= 20}
                onClick={(e) => openDraft(null, e.currentTarget)}
              >
                {t('添加第一个服务', 'Add your first service')}
              </Button>
            }
          />
        )
      ) : (
        !error && (
          <p role="status" className="text-sm text-muted-foreground">
            {t('正在读取服务…', 'Loading services…')}
          </p>
        )
      )}
      {view?.activeProviderId && (
        <Button
          variant="ghost"
          size="sm"
          disabled={disabled}
          onClick={() =>
            void action('select_provider', {
              expectedRevision: view.revision,
              providerId: null,
            }).catch(() => {})
          }
        >
          {t('取消选择 AI 服务', 'Deselect AI service')}
        </Button>
      )}
      <Dialog
        open={!!draft}
        onOpenChange={(open) => {
          if (!open && !busy) {
            if (keyField.current) keyField.current.value = ''
            setDraft(null)
          }
        }}
      >
        <DialogContent
          closeLabel={t('关闭', 'Close')}
          className="max-h-[85vh] overflow-y-auto sm:max-w-[620px]"
          showCloseButton={!busy}
          onCloseAutoFocus={(event) => {
            event.preventDefault()
            restoreFocus()
          }}
          onEscapeKeyDown={(event) => {
            if (busy) event.preventDefault()
          }}
          onInteractOutside={(event) => {
            if (busy) event.preventDefault()
          }}
        >
          <DialogHeader>
            <DialogTitle>
              {draft?.original
                ? t('编辑 AI 服务', 'Edit AI service')
                : t('添加 AI 服务', 'Add AI service')}
            </DialogTitle>
            <DialogDescription>
              {t(
                '保存后主动测试连接。测试只发送固定句子，不发送你的听写或历史。',
                'Test the connection after saving. Tests send only a fixed sentence, never your dictation or history.',
              )}
            </DialogDescription>
          </DialogHeader>
          {draft && (
            <form
              className="space-y-5"
              onSubmit={(event) => {
                event.preventDefault()
                void save()
              }}
            >
              <fieldset disabled={busy} className="space-y-5">
                <div className="space-y-3">
                  <Label>{t('服务类型', 'Service type')}</Label>
                  <Choice
                    className="w-full"
                    label={t('服务类型', 'Service type')}
                    value={draft.protocol.kind}
                    options={Object.entries(providerNames)}
                    onChange={(value) => {
                      const kind = value as ProviderProtocol['kind']
                      if (keyField.current) keyField.current.value = ''
                      setDraft({
                        ...draft,
                        protocol:
                          kind === 'openai_compatible'
                            ? { kind, budget_field: 'max_tokens', instruction_role: 'system' }
                            : { kind },
                        endpoint: endpoints[kind],
                        displayName: providerNames[kind],
                        credentialIntent: 'replace',
                      })
                    }}
                  />
                </div>
                <div className="grid gap-4 sm:grid-cols-2">
                  <div className="space-y-3">
                    <Label htmlFor="provider-name">{t('显示名称', 'Display name')}</Label>
                    <Input
                      id="provider-name"
                      maxLength={160}
                      required
                      value={draft.displayName}
                      onChange={(e) => setDraft({ ...draft, displayName: e.target.value })}
                    />
                  </div>
                  <div className="space-y-3">
                    <Label htmlFor="provider-model">{t('模型 ID', 'Model ID')}</Label>
                    <Input
                      id="provider-model"
                      autoCapitalize="off"
                      spellCheck={false}
                      required
                      maxLength={256}
                      placeholder={t('账户实际可用的模型', 'Exact available model ID')}
                      value={draft.model}
                      onChange={(e) => setDraft({ ...draft, model: e.target.value })}
                    />
                  </div>
                </div>
                <div className="space-y-3">
                  <Label htmlFor="provider-endpoint">
                    {t('完整服务地址', 'Full endpoint URL')}
                  </Label>
                  <Input
                    id="provider-endpoint"
                    autoCapitalize="off"
                    spellCheck={false}
                    type="url"
                    required
                    maxLength={2048}
                    value={draft.endpoint}
                    onChange={(e) => setDraft({ ...draft, endpoint: e.target.value })}
                  />
                  <p className="text-xs leading-5 text-muted-foreground">
                    {t(
                      '填写实际接收请求的完整地址，不要在地址中加入密钥。',
                      'Enter the full request endpoint. Do not put a key in the URL.',
                    )}
                  </p>
                </div>
                {draft.protocol.kind === 'openai_compatible' && (
                  <div className="grid gap-4 sm:grid-cols-2">
                    <div className="space-y-3">
                      <Label>{t('输出预算参数', 'Output budget parameter')}</Label>
                      <Choice
                        className="w-full"
                        label={t('输出预算参数', 'Output budget parameter')}
                        value={draft.protocol.budget_field}
                        options={[
                          ['max_tokens', 'max_tokens'],
                          ['max_completion_tokens', 'max_completion_tokens'],
                        ]}
                        onChange={(value) => {
                          if (draft.protocol.kind === 'openai_compatible')
                            setDraft({
                              ...draft,
                              protocol: {
                                ...draft.protocol,
                                budget_field: value as 'max_tokens' | 'max_completion_tokens',
                              },
                            })
                        }}
                      />
                    </div>
                    <div className="space-y-3">
                      <Label>{t('编辑指令角色', 'Instruction role')}</Label>
                      <Choice
                        className="w-full"
                        label={t('编辑指令角色', 'Instruction role')}
                        value={draft.protocol.instruction_role}
                        options={[
                          ['system', 'system'],
                          ['developer', 'developer'],
                        ]}
                        onChange={(value) => {
                          if (draft.protocol.kind === 'openai_compatible')
                            setDraft({
                              ...draft,
                              protocol: {
                                ...draft.protocol,
                                instruction_role: value as 'system' | 'developer',
                              },
                            })
                        }}
                      />
                    </div>
                  </div>
                )}
                {local ? (
                  <p className="rounded-lg bg-muted p-3 text-sm leading-6">
                    {t(
                      '本机 Ollama 无需密钥。请先启动服务并安装本地模型；该服务是否继续联网由其自身配置决定。SpeakMore 不会自动下载 Ollama 模型。',
                      'Ollama on this device needs no key. Start the service and install a local model first. Its own settings control any onward network access. SpeakMore does not download Ollama models.',
                    )}
                  </p>
                ) : (
                  <div className="space-y-3">
                    <Label>{t('API Key', 'API key')}</Label>
                    <Choice
                      className="w-full"
                      label={t('密钥处理方式', 'Key action')}
                      value={draft.credentialIntent}
                      options={[
                        ...(hasExistingKey
                          ? [['keep', t('保留已保存密钥', 'Keep saved key')] as [string, string]]
                          : []),
                        [
                          'replace',
                          hasExistingKey ? t('更换密钥', 'Replace key') : t('保存密钥', 'Save key'),
                        ],
                        ['remove', t('不保存密钥', 'Do not save a key')],
                      ]}
                      onChange={(value) => {
                        if (keyField.current) keyField.current.value = ''
                        setDraft({ ...draft, credentialIntent: value as CredentialIntent })
                      }}
                    />
                    {draft.credentialIntent === 'replace' && (
                      <>
                        <Input
                          key="private-key"
                          ref={keyField}
                          aria-label="API Key"
                          type="password"
                          autoComplete="new-password"
                          autoCapitalize="off"
                          spellCheck={false}
                          maxLength={8192}
                          required
                          placeholder={t('输入后保存到系统钥匙串', 'Save to the system keychain')}
                        />
                        <p className="text-xs leading-5 text-muted-foreground">
                          {t(
                            '提交后立即清空此输入框，密钥不会再次显示。失败后请重新输入。',
                            'This field clears immediately on submission. Saved keys are never shown again. Re-enter the key if saving fails.',
                          )}
                        </p>
                      </>
                    )}
                  </div>
                )}
                {!local && (
                  <p className="text-xs leading-5 text-muted-foreground">
                    {t(
                      '使用此服务时仅发送文字，音频仍在本机处理。',
                      'Only text is sent when using this service. Audio stays on this device.',
                    )}
                  </p>
                )}
              </fieldset>
              <InlineError message={providerErrorMessage(draftError, t)} />
              {!!draftError && (
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  disabled={busy}
                  onClick={() => {
                    if (keyField.current) keyField.current.value = ''
                    setDraft(null)
                    void reload().catch(() => {})
                  }}
                >
                  <ReloadIcon aria-hidden />
                  {t('关闭并重新读取配置', 'Close and reload settings')}
                </Button>
              )}
              <DialogFooter>
                <Button
                  type="button"
                  variant="outline"
                  disabled={busy}
                  onClick={() => {
                    if (keyField.current) keyField.current.value = ''
                    setDraft(null)
                  }}
                >
                  {t('取消', 'Cancel')}
                </Button>
                <Button type="submit" disabled={busy}>
                  {busy ? t('正在保存…', 'Saving…') : t('保存服务', 'Save service')}
                </Button>
              </DialogFooter>
            </form>
          )}
        </DialogContent>
      </Dialog>
      <Confirm
        open={!!recovering}
        confirmLabel={
          recovering?.row.kind === 'credential'
            ? t('忘记记录', 'Forget record')
            : t('移除配置', 'Remove configuration')
        }
        pendingLabel={t('正在处理…', 'Processing…')}
        onOpenChange={(open) => {
          if (!open) setRecovering(null)
        }}
        restoreFocus={restoreFocus}
        title={
          recovering?.row.kind === 'credential'
            ? t('忘记无效凭据记录', 'Forget invalid credential record')
            : t('移除不可用的服务配置', 'Remove unusable service configuration')
        }
        description={
          recovering?.row.kind === 'credential'
            ? t(
                '仅移除这条本地清理记录，不会删除任何系统钥匙串密钥，也不代表旧密钥已清除。请先在系统钥匙串中检查 SpeakMore 项目。',
                'This only removes the local cleanup record. It does not delete any system key or confirm that an old key is gone. Check SpeakMore entries in your system keychain first.',
              )
            : t(
                '移除此服务配置。若正在使用，将取消选择并关闭文字整理。可确认归属的旧密钥会进入清理；无法识别的凭据会保留为待处理记录。模型、听写历史与其他配置保留。',
                'Remove this service. If selected, it will be deselected and text refinement turned off. Identifiable keys enter cleanup; unrecognized credentials remain for review. Models, history and other settings are kept.',
              )
        }
        onConfirm={() =>
          recovering && view
            ? action(
                'discard_invalid_provider',
                {
                  expectedRevision: view.revision,
                  recoveryId: recovering.row.recoveryId,
                },
                t(
                  '已处理此配置，请检查剩余待处理记录。',
                  'Record processed. Review any remaining entries.',
                ),
              ).catch(async (e) => {
                // Reload so a retry after a conflict uses the current revision;
                // keep the native error so the dialog explains it.
                await reload().catch(() => {})
                throw e
              })
            : Promise.resolve()
        }
      />
      <Confirm
        open={!!removing}
        onOpenChange={(open) => {
          if (!open) setRemoving(null)
        }}
        restoreFocus={restoreFocus}
        title={t(
          `删除 ${removing?.row.displayName ?? ''}`,
          `Delete ${removing?.row.displayName ?? ''}`,
        )}
        description={t(
          '删除此服务配置，并清理对应密钥。若系统清理失败，页面会保留重试入口。模型文件和听写记录不受影响。',
          'Delete this service and remove its key. If system cleanup fails, a retry action remains. Local models and dictation history are unaffected.',
        )}
        onConfirm={() =>
          removing && view
            ? action('delete_provider', {
                expectedRevision: view.revision,
                providerId: removing.row.providerId,
              }).catch(async (e) => {
                await reload().catch(() => {})
                throw e
              })
            : Promise.resolve()
        }
      />
    </div>
  )
}
