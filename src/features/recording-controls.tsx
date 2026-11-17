import { asAppError, type UiMessage } from '@/lib/errors'
import { useCallback, useEffect, useRef, useState } from 'react'
import { isTauri } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { call, useApp, useText } from '@/lib/app-state'
import {
  type RecordingControls as Controls,
  type ShortcutBinding,
  type ShortcutPreferences,
} from '@/lib/types'
import { shortcutDescription, shortcutLabels } from '@/lib/shortcut-input'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { ShortcutRecorder } from './shortcut-recorder'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from '@/components/ui/dialog'
import { SettingsRow, InlineError } from '@/components/common'

type Editing = {
  id: string
  action: 'push_to_talk'
  binding: ShortcutBinding
  expected: ShortcutPreferences
}
function Keys({ accelerator }: { accelerator: string }) {
  return (
    <span
      className="inline-flex flex-wrap justify-center gap-1.5"
      aria-label={shortcutDescription(accelerator)}
    >
      {shortcutLabels(accelerator).map((key, i) => (
        <kbd
          key={i}
          aria-hidden
          className="min-w-8 rounded-md border bg-card px-2 py-1.5 text-center text-sm font-medium shadow-xs"
        >
          {key}
        </kbd>
      ))}
    </span>
  )
}
export function RecordingControls() {
  const { data, requestPermissionRestart } = useApp(),
    t = useText()
  const [controls, setControls] = useState<Controls | null>(null)
  const [error, setError] = useState<UiMessage | null>(null)
  const [loadError, setLoadError] = useState<UiMessage | null>(null)
  const [editing, setEditing] = useState<Editing | null>(null)
  const [editError, setEditError] = useState<UiMessage | null>(null)
  const [pending, setPending] = useState(false)
  const trigger = useRef<HTMLButtonElement | null>(null)
  const mounted = useRef(true)
  const merge = useCallback((next: Controls) => {
    if (mounted.current) setControls((old) => (!old || next.revision >= old.revision ? next : old))
  }, [])
  const load = useCallback(async () => {
    try {
      const next = await call<Controls>('get_recording_controls')
      merge(next)
      if (mounted.current) setLoadError(null)
    } catch (e) {
      if (mounted.current) setLoadError(asAppError(e))
    }
  }, [merge])
  useEffect(() => {
    mounted.current = true
    let disposed = false,
      off: UnlistenFn | undefined
    void (async () => {
      try {
        if (isTauri())
          off = await listen('controls_changed', () => {
            if (!disposed) void load()
          })
        if (disposed) off?.()
      } catch (e) {
        if (!disposed) setError(asAppError(e))
      } finally {
        // Read the current controls even if change events are unavailable.
        if (!disposed) await load()
      }
    })()
    return () => {
      disposed = true
      mounted.current = false
      off?.()
    }
  }, [load])
  useEffect(() => {
    if (!editing) return
    const id = editing.id
    const timer = setInterval(() => {
      void call('shortcut_edit_heartbeat', { editId: id, close: false }).catch((e) => {
        if (mounted.current) {
          setEditing((old) => (old?.id === id ? null : old))
          setError(asAppError(e))
        }
      })
    }, 10_000)
    return () => {
      clearInterval(timer)
      void call('shortcut_edit_heartbeat', { editId: id, close: true }).catch(() => {})
    }
  }, [editing?.id])
  const begin = async (button: HTMLButtonElement) => {
    if (!controls) return
    setError(null)
    setEditError(null)
    setPending(true)
    trigger.current = button
    try {
      const id = await call<string>('begin_shortcut_edit')
      if (!mounted.current) {
        await call('shortcut_edit_heartbeat', { editId: id, close: true })
        return
      }
      const preferences = controls.preferences
      setEditing({
        id,
        action: 'push_to_talk',
        binding: { ...preferences.pushToTalk },
        expected: preferences,
      })
    } catch (e) {
      if (mounted.current) setError(asAppError(e))
    } finally {
      if (mounted.current) setPending(false)
    }
  }
  const save = async () => {
    if (!editing) return
    setPending(true)
    setEditError(null)
    try {
      merge(
        await call<Controls>('save_recording_shortcut', {
          editId: editing.id,
          action: editing.action,
          binding: editing.binding,
          expected: editing.expected,
        }),
      )
      setEditing(null)
    } catch (e) {
      if (mounted.current) setEditError(asAppError(e))
    } finally {
      if (mounted.current) setPending(false)
    }
  }
  const perform = async (command: string, args = {}) => {
    setError(null)
    setPending(true)
    try {
      const result = await call<Controls>(command, args)
      if (command === 'retry_recording_shortcut') merge(result)
    } catch (e) {
      setError(asAppError(e))
    } finally {
      setPending(false)
    }
  }
  const unavailable =
    !data?.desktop ||
    pending ||
    !!data?.session.sessionId ||
    data?.captureSuspended ||
    controls?.serviceError?.code === 'ControlsUnavailable'
  return (
    <>
      {!controls && loadError && (
        <SettingsRow
          title={t('听写快捷键', 'Dictation shortcut')}
          description={t('按一下说话，再按一下输入。', 'Press once to speak, again to insert.')}
          feedback={<InlineError message={loadError} />}
        >
          <Button
            variant="outline"
            disabled={pending}
            onClick={async () => {
              setPending(true)
              await load()
              if (mounted.current) setPending(false)
            }}
          >
            {t('重试', 'Retry')}
          </Button>
        </SettingsRow>
      )}
      {controls?.bindings
        .filter((binding) => binding.action === 'push_to_talk')
        .map((binding) => (
          <SettingsRow
            key={binding.action}
            title={t('听写快捷键', 'Dictation shortcut')}
            description={t('按一下说话，再按一下输入。', 'Press once to speak, again to insert.')}
            feedback={
              binding.error &&
              (binding.error.code === 'FnPermissionRequired' ? (
                <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg bg-muted/50 px-4 py-3">
                  <p
                    role="status"
                    className="min-w-0 flex-1 text-sm leading-6 text-muted-foreground"
                  >
                    {data?.permissions.accessibility
                      ? t(
                          '辅助功能权限已开启，请重启 SpeakMore 以启用快捷键。',
                          'Accessibility access is allowed. Restart SpeakMore to enable the shortcut.',
                        )
                      : t(
                          '请在上方开启辅助功能权限，然后重启 SpeakMore，无需重新保存快捷键。',
                          'Allow Accessibility above, then restart SpeakMore. No need to save your shortcut again.',
                        )}
                  </p>
                  <Button
                    variant="outline"
                    className="shrink-0"
                    disabled={
                      pending || !!editing || !!data?.session.sessionId || data?.captureSuspended
                    }
                    onClick={requestPermissionRestart}
                  >
                    {t('重启 SpeakMore', 'Restart SpeakMore')}
                  </Button>
                </div>
              ) : (
                <InlineError message={binding.error} />
              ))
            }
          >
            <div className="grid justify-items-end gap-1.5">
              <Button
                variant="outline"
                className="h-auto gap-3 p-2"
                disabled={unavailable}
                onClick={(event) => void begin(event.currentTarget)}
                aria-label={t('修改听写快捷键', 'Edit dictation shortcut')}
              >
                <Keys accelerator={binding.accelerator} />
                <span className="px-1">{t('修改', 'Edit')}</span>
              </Button>
              {(!binding.enabled || !binding.registered) && (
                <span className="text-xs text-muted-foreground">
                  {!binding.enabled
                    ? t('已关闭', 'Off')
                    : binding.accelerator === 'Fn' && !data?.permissions.accessibility
                      ? t('需要授权', 'Permission required')
                      : binding.error?.code === 'FnPermissionRequired'
                        ? t('重启后启用', 'Restart to enable')
                        : t('快捷键不可用', 'Shortcut unavailable')}
                </span>
              )}
            </div>
          </SettingsRow>
        ))}
      {(controls?.error || controls?.serviceError) &&
        controls?.error?.code !== 'FnPermissionRequired' && (
          <InlineError message={controls.error || controls.serviceError || ''} />
        )}
      {data?.desktop &&
        controls &&
        ((controls.error && controls.error.code !== 'FnPermissionRequired') ||
          controls.bindings.some(
            (b) => b.enabled && !b.registered && b.error?.code !== 'FnPermissionRequired',
          )) && (
          <Button
            className="mt-3"
            variant="outline"
            disabled={pending || !!editing || !!data.session.sessionId}
            onClick={() => void perform('retry_recording_shortcut')}
          >
            {t('重试', 'Retry')}
          </Button>
        )}
      <SettingsRow
        title={t('悬浮录音条', 'Recording bar')}
        description={t(
          '显示录音状态与完成、取消按钮。',
          'Shows recording status, with finish and cancel controls.',
        )}
      >
        <Button
          variant="outline"
          disabled={unavailable || !controls?.hudAvailable || !!editing}
          onClick={() => void perform('preview_recording_panel', { show: true })}
        >
          {t('预览悬浮条', 'Preview recording bar')}
        </Button>
      </SettingsRow>
      {(error || (controls && loadError)) && <InlineError message={error || loadError} />}
      <Dialog
        open={!!editing}
        onOpenChange={(open) => {
          if (!open && !pending) setEditing(null)
        }}
      >
        {editing && (
          <DialogContent
            closeLabel={t('关闭', 'Close')}
            showCloseButton={!pending}
            onEscapeKeyDown={(event) => {
              if (pending) event.preventDefault()
            }}
            onCloseAutoFocus={(event) => {
              event.preventDefault()
              trigger.current?.focus()
            }}
          >
            <DialogHeader>
              <DialogTitle>{t('听写快捷键', 'Dictation shortcut')} </DialogTitle>
              <DialogDescription>
                {t('直接按下 Fn 或组合键。', 'Press Fn or a key combination.')}
              </DialogDescription>
            </DialogHeader>
            {editing && (
              <>
                <ShortcutRecorder
                  editId={editing.id}
                  onError={setEditError}
                  value={editing.binding.accelerator}
                  disabled={pending}
                  onChange={(accelerator) => {
                    setEditError(null)
                    setEditing((old) => old && { ...old, binding: { ...old.binding, accelerator } })
                  }}
                />
                <label className="flex items-center justify-between gap-4 py-2 text-sm">
                  {t('启用这个快捷键', 'Enable this shortcut')}
                  <Switch
                    checked={editing.binding.enabled}
                    disabled={pending}
                    onCheckedChange={(enabled) =>
                      setEditing((old) => old && { ...old, binding: { ...old.binding, enabled } })
                    }
                  />
                </label>
                {editError && <InlineError message={editError} />}
                <DialogFooter>
                  <Button
                    variant="ghost"
                    className="sm:mr-auto"
                    disabled={pending}
                    onClick={() => {
                      setEditError(null)
                      setEditing(
                        (old) =>
                          old && {
                            ...old,
                            binding: {
                              ...old.binding,
                              accelerator: 'Fn',
                            },
                          },
                      )
                    }}
                  >
                    {t('恢复默认', 'Reset')}
                  </Button>
                  <Button variant="outline" disabled={pending} onClick={() => setEditing(null)}>
                    {t('取消', 'Cancel')}
                  </Button>
                  <Button disabled={pending} onClick={() => void save()}>
                    {pending ? t('正在保存…', 'Saving…') : t('保存', 'Save')}
                  </Button>
                </DialogFooter>
              </>
            )}
          </DialogContent>
        )}
      </Dialog>
    </>
  )
}
