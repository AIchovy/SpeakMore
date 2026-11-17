import { useEffect, useRef, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { call, useText } from '@/lib/app-state'
import { asAppError, type UiMessage } from '@/lib/errors'
import { shortcutDescription, shortcutLabels } from '@/lib/shortcut-input'

type Capture = {
  editId: string
  captureId: string
  kind: 'captured' | 'invalid' | 'cancelled' | 'interrupted'
  accelerator?: string
}

export function ShortcutRecorder({
  editId,
  value,
  disabled,
  onChange,
  onError,
}: {
  editId: string
  value: string
  disabled: boolean
  onChange: (value: string) => void
  onError: (error: UiMessage | null) => void
}) {
  const t = useText()
  const [phase, setPhase] = useState<'idle' | 'starting' | 'listening'>('idle')
  const [subscription, setSubscription] = useState<'pending' | 'ready' | 'failed'>('pending')
  // Bumped to subscribe again after the capture event listener failed.
  const [attempt, setAttempt] = useState(0)
  const ready = subscription === 'ready'
  const active = useRef<string | null>(null)
  const live = useRef(false)
  const handlers = useRef({ onChange, onError, t })
  handlers.current = { onChange, onError, t }

  const stop = () => {
    const captureId = active.current
    active.current = null
    if (captureId)
      void call('set_shortcut_capture', { editId, captureId, active: false }).catch(() => {})
  }
  const arm = async () => {
    if (!ready || disabled) return
    stop()
    const captureId = crypto.randomUUID()
    active.current = captureId
    setPhase('starting')
    handlers.current.onError(null)
    try {
      await call('set_shortcut_capture', { editId, captureId, active: true })
      if (live.current && active.current === captureId) setPhase('listening')
      else void call('set_shortcut_capture', { editId, captureId, active: false }).catch(() => {})
    } catch (error) {
      if (live.current && active.current === captureId) {
        active.current = null
        setPhase('idle')
        handlers.current.onError(asAppError(error))
      }
    }
  }
  useEffect(() => {
    live.current = true
    let disposed = false
    setSubscription('pending')
    const subscription = listen<Capture>('shortcut_capture', ({ payload }) => {
      if (disposed || payload.editId !== editId || payload.captureId !== active.current) return
      active.current = null
      setPhase('idle')
      if (payload.kind === 'captured' && payload.accelerator) {
        handlers.current.onError(null)
        handlers.current.onChange(payload.accelerator)
      } else if (payload.kind === 'invalid') {
        handlers.current.onError(
          handlers.current.t(
            '请按 Fn，或使用 Control、Option、Command 加一个键。',
            'Press Fn, or combine a key with Control, Option or Command.',
          ),
        )
      } else if (payload.kind === 'interrupted') {
        handlers.current.onError(
          handlers.current.t(
            '按键拦截已中断，请点击重新录入。',
            'Key capture was interrupted. Click to try again.',
          ),
        )
      }
    })
    void subscription
      .then((off) => {
        if (disposed) off()
        else setSubscription('ready')
      })
      .catch((error) => {
        if (disposed) return
        setSubscription('failed')
        handlers.current.onError(asAppError(error))
      })
    const blur = () => {
      stop()
      if (!disposed) setPhase('idle')
    }
    window.addEventListener('blur', blur)
    return () => {
      disposed = true
      live.current = false
      stop()
      window.removeEventListener('blur', blur)
      void subscription.then((off) => off()).catch(() => {})
    }
  }, [editId, attempt])
  useEffect(() => {
    if (ready) void arm()
  }, [ready])
  useEffect(() => {
    if (disabled) {
      stop()
      setPhase('idle')
    }
  }, [disabled])

  return (
    <button
      type="button"
      disabled={disabled || subscription === 'pending' || phase === 'starting'}
      onClick={(event) => {
        event.currentTarget.focus()
        if (subscription === 'failed') {
          handlers.current.onError(null)
          setAttempt((n) => n + 1)
        } else void arm()
      }}
      aria-label={t('录入快捷键', 'Record shortcut')}
      className="flex min-h-28 flex-col items-center justify-center gap-3 rounded-lg border bg-muted/30 p-5 outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-60"
    >
      <span className="inline-flex gap-1.5" aria-label={shortcutDescription(value)}>
        {shortcutLabels(value).map((key, index) => (
          <kbd
            key={index}
            className="min-w-8 rounded-md border bg-card px-2 py-1.5 text-sm font-medium shadow-xs"
          >
            {key}
          </kbd>
        ))}
      </span>
      <span className="text-xs text-muted-foreground" aria-live="polite">
        {phase === 'listening'
          ? t('请按快捷键，松开后完成录入', 'Press a shortcut, then release to finish')
          : phase === 'starting'
            ? t('正在准备…', 'Preparing…')
            : subscription === 'failed'
              ? t('点击重试', 'Click to try again')
              : t('点击重新录入', 'Click to record')}
      </span>
    </button>
  )
}
