import { asAppError, type UiMessage } from '@/lib/errors'
import { useEffect, useRef, useState } from 'react'
import { call, useApp, useText } from '@/lib/app-state'
import { type AppChoice, type AppRule, type AppRulesSnapshot } from '@/lib/types'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Confirm, EmptyState, InlineError, SettingsRow } from '@/components/common'
import { EditIcon, MonitorIcon, PlusIcon, ShieldIcon, TrashIcon } from '@/components/icons'

type Draft = {
  expectedRevision: number
  appIdentity: string
  displayName: string
  selectionId: string | null
  manualOnly: boolean
  historyExcluded: boolean
}

export function RuleRows({
  items,
  disabled,
  edit,
  remove,
}: {
  items: AppRule[]
  disabled: boolean
  edit: (r: AppRule, trigger: HTMLButtonElement) => void
  remove: (r: AppRule, trigger: HTMLButtonElement) => void
}) {
  const t = useText()
  return (
    <div className="divide-y overflow-hidden rounded-xl border bg-card">
      {items.map((rule) => (
        <div key={rule.appIdentity} className="flex flex-wrap items-center gap-4 px-5 py-5">
          <MonitorIcon aria-hidden className="size-5 shrink-0 text-muted-foreground" />
          <div className="min-w-0 flex-1 basis-64">
            <h3 className="break-words font-medium">
              {rule.displayName || t('未命名应用', 'Unnamed app')}
            </h3>
            <div className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
              <span>
                {rule.manualOnly
                  ? t('始终手动粘贴', 'Always paste manually')
                  : t('跟随全局交付设置', 'Use global delivery settings')}
              </span>
              <span className={rule.historyExcluded ? 'text-accent-foreground' : undefined}>
                {rule.historyExcluded
                  ? t('不保存历史', 'Never save history')
                  : t('跟随全局历史设置', 'Use global history settings')}
              </span>
            </div>
          </div>
          <div className="flex gap-1">
            <Button
              variant="ghost"
              size="sm"
              disabled={disabled}
              onClick={(event) => edit(rule, event.currentTarget)}
              aria-label={t(
                `编辑 ${rule.displayName} 的规则`,
                `Edit rules for ${rule.displayName}`,
              )}
            >
              <EditIcon aria-hidden />
              {t('编辑', 'Edit')}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              disabled={disabled}
              onClick={(event) => remove(rule, event.currentTarget)}
              aria-label={t(
                `移除 ${rule.displayName} 的规则`,
                `Remove rules for ${rule.displayName}`,
              )}
            >
              <TrashIcon aria-hidden />
              {t('移除', 'Remove')}
            </Button>
          </div>
        </div>
      ))}
    </div>
  )
}

export function AppRules() {
  const { data } = useApp(),
    t = useText()
  const [rules, setRules] = useState<AppRulesSnapshot | null>(null)
  const [reload, setReload] = useState(0),
    [loading, setLoading] = useState(true),
    [busy, setBusy] = useState(false)
  const [error, setError] = useState<UiMessage | null>(null),
    [draftError, setDraftError] = useState<UiMessage | null>(null)
  const [draft, setDraft] = useState<Draft | null>(null),
    [removing, setRemoving] = useState<(AppRule & { expectedRevision: number }) | null>(null)
  const trigger = useRef<HTMLElement | null>(null),
    addButton = useRef<HTMLButtonElement | null>(null)
  const mounted = useRef(true)
  const nativeFocusPending = useRef(false)
  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])
  const accept = (next: AppRulesSnapshot) =>
    setRules((current) => (current && current.revision > next.revision ? current : next))
  useEffect(() => {
    let disposed = false
    setLoading(true)
    void call<AppRulesSnapshot>('list_app_rules')
      .then((next) => {
        if (!disposed) {
          accept(next)
          setError(null)
        }
      })
      .catch((e) => {
        if (!disposed) setError(asAppError(e))
      })
      .finally(() => {
        if (!disposed) setLoading(false)
      })
    return () => {
      disposed = true
    }
  }, [data?.settings.revision, reload])
  const rememberFocus = (element?: HTMLElement) => {
    trigger.current = element ?? addButton.current
  }
  const restoreFocus = () => {
    ;(trigger.current?.isConnected ? trigger.current : addButton.current)?.focus()
  }
  useEffect(() => {
    if (!busy && !loading && !draft && nativeFocusPending.current) {
      nativeFocusPending.current = false
      restoreFocus()
    }
  }, [busy, loading, draft])
  async function choose() {
    if (busy) return
    if (!draft) rememberFocus()
    setBusy(true)
    setError(null)
    setDraftError(null)
    let openedDraft = false
    try {
      const choice = await call<AppChoice | null>('choose_app_for_rule')
      if (choice && mounted.current) {
        const latest = await call<AppRulesSnapshot>('list_app_rules')
        if (!mounted.current) return
        accept(latest)
        openedDraft = true
        const existing = latest.items.find((r) => r.appIdentity === choice.appIdentity)
        setDraft({
          ...choice,
          expectedRevision: latest.revision,
          manualOnly: existing?.manualOnly ?? true,
          historyExcluded: existing?.historyExcluded ?? false,
        })
      }
    } catch (e) {
      if (mounted.current) {
        if (draft) setDraftError(asAppError(e))
        else setError(asAppError(e))
      }
    } finally {
      if (mounted.current) {
        nativeFocusPending.current = !openedDraft && !draft
        setBusy(false)
      }
    }
  }
  async function save() {
    if (!draft || !rules || busy) return
    setBusy(true)
    setDraftError(null)
    try {
      const next = await call<AppRulesSnapshot>('upsert_app_rule', {
        input: {
          expectedRevision: draft.expectedRevision,
          selectionId: draft.selectionId,
          appIdentity: draft.selectionId ? null : draft.appIdentity,
          manualOnly: draft.manualOnly,
          historyExcluded: draft.historyExcluded,
        },
      })
      if (mounted.current) {
        accept(next)
        setDraft(null)
      }
    } catch (e) {
      if (mounted.current) {
        setDraftError(asAppError(e))
        setReload((v) => v + 1)
      }
    } finally {
      if (mounted.current) setBusy(false)
    }
  }
  if (!data) return null
  return (
    <>
      <div className="mb-5 flex flex-wrap items-center justify-between gap-4">
        <div>
          <h2 className="text-lg font-semibold">{t('应用规则', 'App rules')}</h2>
          <p className="mt-1 text-sm text-muted-foreground">
            {t(
              '按应用决定如何交付文字、是否保存历史。',
              'Choose delivery and history behavior for each app.',
            )}
          </p>
        </div>
        <Button
          ref={addButton}
          variant="outline"
          disabled={!data.desktop || busy || loading || !rules || rules.items.length >= 100}
          onClick={() => void choose()}
        >
          <PlusIcon aria-hidden />
          {busy && !draft ? t('正在选择…', 'Choosing…') : t('添加应用', 'Add app')}
        </Button>
      </div>
      <p className="mb-5 text-sm leading-6 text-muted-foreground">
        {t(
          '快捷键和菜单栏听写按开始时的来源应用匹配规则；主窗口听写使用 SpeakMore 的规则。交付与历史规则仅按应用匹配。听写整理可另设应用与网站提示词。关闭手动限制仍需通过输入兼容性检查。',
          'Shortcut and menu recordings use the app active at the start; main-window recordings use SpeakMore’s rules. Delivery and history rules match apps only. Dictation refinement has separate app and website prompts. Compatibility checks still apply.',
        )}
      </p>
      {!data.desktop && (
        <p className="mb-4 text-sm text-muted-foreground">
          {t(
            '请在 macOS 应用中选择和管理规则。',
            'Select and manage apps in the macOS desktop app.',
          )}
        </p>
      )}
      <InlineError message={error} />
      {error && (
        <Button
          variant="outline"
          className="mb-4"
          disabled={busy || loading}
          onClick={() => setReload((v) => v + 1)}
        >
          {t('重新读取', 'Retry')}
        </Button>
      )}
      {loading && !rules ? (
        <p role="status" className="py-8 text-sm text-muted-foreground">
          {t('正在读取应用规则…', 'Loading app rules…')}
        </p>
      ) : (
        rules &&
        (rules.items.length ? (
          <RuleRows
            items={rules.items}
            disabled={busy || loading}
            edit={(rule, button) => {
              rememberFocus(button)
              setDraftError(null)
              setDraft({ ...rule, selectionId: null, expectedRevision: rules.revision })
            }}
            remove={(rule, button) => {
              rememberFocus(button)
              setRemoving({ ...rule, expectedRevision: rules.revision })
            }}
          />
        ) : (
          <EmptyState
            icon={ShieldIcon}
            title={t('为常用应用保留选择', 'Set preferences for the apps you use')}
            description={t(
              '例如，在工作聊天中手动粘贴，或让私密应用中的转写不进入历史。添加应用后可分别设置。',
              'For example, paste manually in work chat or keep transcripts from a private app out of history. Add an app to choose its rules.',
            )}
          />
        ))
      )}
      <p className="mt-5 text-xs leading-5 text-muted-foreground">
        {t(
          '“不保存历史”不会清除已有记录；最近结果仍临时保留。无法确认来源时，有历史排除规则便不保存本次历史。录音中收紧的规则持续到本次结束，放宽规则从下次生效。',
          'Existing history and temporary recent results are preserved. If the source is unknown and any app excludes history, this recording is not saved. Stricter rules apply now; relaxed rules apply next time.',
        )}
      </p>
      <Dialog
        open={!!draft}
        onOpenChange={(open) => {
          if (!open && !busy) {
            setDraft(null)
            setDraftError(null)
          }
        }}
      >
        <DialogContent
          className="max-w-lg"
          closeLabel={t('关闭', 'Close')}
          onCloseAutoFocus={(e) => {
            e.preventDefault()
            restoreFocus()
          }}
        >
          <DialogHeader>
            <DialogTitle>{draft?.displayName || t('应用规则', 'App rules')}</DialogTitle>
            <DialogDescription>
              {t(
                '保存后生效，不会更改全局设置。',
                'Saving applies these rules without changing global settings.',
              )}
            </DialogDescription>
          </DialogHeader>
          {draft && (
            <div>
              <SettingsRow
                title={t('始终手动粘贴', 'Always paste manually')}
                description={t(
                  '显示结果，由你复制后粘贴到正确位置。',
                  'Show the result so you can copy and paste it in the right place.',
                )}
              >
                <Switch
                  aria-label={t('始终手动粘贴', 'Always paste manually')}
                  checked={draft.manualOnly}
                  disabled={busy}
                  onCheckedChange={(v) => setDraft({ ...draft, manualOnly: v })}
                />
              </SettingsRow>
              <SettingsRow
                title={t('不保存该应用历史', 'Never save history for this app')}
                description={t(
                  '即使全局历史开启，也不保存从这个应用发起的转写。',
                  'Do not save transcripts started in this app, even when global history is on.',
                )}
              >
                <Switch
                  aria-label={t('不保存该应用历史', 'Never save history for this app')}
                  checked={draft.historyExcluded}
                  disabled={busy}
                  onCheckedChange={(v) => setDraft({ ...draft, historyExcluded: v })}
                />
              </SettingsRow>
            </div>
          )}
          <InlineError message={draftError} />
          {draftError && (
            <Button variant="ghost" disabled={busy} onClick={() => void choose()}>
              {t('重新选择并载入最新规则', 'Choose app and reload saved rules')}
            </Button>
          )}
          <DialogFooter>
            <Button variant="outline" disabled={busy} onClick={() => setDraft(null)}>
              {t('取消', 'Cancel')}
            </Button>
            <Button disabled={busy || loading || !rules} onClick={() => void save()}>
              {busy ? t('正在保存…', 'Saving…') : t('保存规则', 'Save rules')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Confirm
        open={!!removing}
        title={t(
          `移除 ${removing?.displayName ?? ''} 的规则？`,
          `Remove rules for ${removing?.displayName ?? ''}?`,
        )}
        description={t(
          '后续听写将跟随全局设置，不会删除已有历史。',
          'Future recordings will use global settings. Existing history is preserved.',
        )}
        onOpenChange={(open) => {
          if (!open) {
            setRemoving(null)
          }
        }}
        restoreFocus={restoreFocus}
        onConfirm={async () => {
          if (!removing || !rules) return
          try {
            accept(
              await call<AppRulesSnapshot>('delete_app_rule', {
                expectedRevision: removing.expectedRevision,
                appIdentity: removing.appIdentity,
              }),
            )
          } catch (e) {
            setReload((v) => v + 1)
            throw e
          }
        }}
      />
    </>
  )
}
