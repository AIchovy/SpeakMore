import { useEffect, useRef, useState } from 'react'
import { isTauri } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { meetingCall, meetingError } from './meetings-api'
import type { Translate } from '@/lib/errors'

export function ExperienceShortcuts({ t }: { t: Translate }) {
  const [state, setState] = useState({ enabled: false, error: null as string | null }),
    [pending, setPending] = useState(false)
  useEffect(() => {
    let alive = true
    void meetingCall<typeof state>('experience_shortcuts_view')
      .then((v) => {
        if (alive) setState(v)
      })
      .catch(() => {})
    return () => {
      alive = false
    }
  }, [])
  return (
    <div className="my-4 rounded-lg border p-4">
      <label className="flex items-center justify-between gap-4 text-sm">
        <span>{t('快捷回顾、助手与重点标记', 'Quick recap, assistant and highlights')}</span>
        <Switch
          aria-label={t('启用体验快捷键', 'Enable meeting and caption shortcuts')}
          checked={state.enabled}
          disabled={pending}
          onCheckedChange={(enabled) => {
            setPending(true)
            void meetingCall<typeof state>('experience_shortcuts', { enabled })
              .then(setState)
              .catch((e) => setState((s) => ({ ...s, error: meetingError(e) })))
              .finally(() => setPending(false))
          }}
        />
      </label>
      <p className="mt-2 text-xs leading-6 text-muted-foreground">
        {t(
          '⌘⇧R 回顾最近 60 秒；⌘⇧J 显示或隐藏字幕助手；⌘⇧M 标记正在录制的会议重点。Windows 使用 Ctrl。',
          '⌘⇧R recaps 60 seconds; ⌘⇧J shows or hides the caption assistant; ⌘⇧M highlights the current meeting. Use Ctrl on Windows.',
        )}
      </p>
      {state.error && (
        <p role="alert" className="mt-2 text-xs text-destructive">
          {state.error}
        </p>
      )}
    </div>
  )
}
interface Reply {
  id: string
  noteMeetingId?: string | null
  phase: string
  original: string
  text: string
  error: string | null
}
export function CaptionShortReply({
  t,
  model,
  language,
  meetingId,
  onSaved,
}: {
  t: Translate
  model: string
  language: string
  meetingId?: string
  onSaved?: () => void
}) {
  const [view, setView] = useState<Reply | null>(null),
    [style, setStyle] = useState('concise'),
    [text, setText] = useState(''),
    [error, setError] = useState(''),
    [starting, setStarting] = useState(false),
    [holding, setHolding] = useState(false),
    [copied, setCopied] = useState(false)
  const saved = useRef(''),
    savedCallback = useRef(onSaved)
  savedCallback.current = onSaved
  const down = useRef(false),
    id = useRef<string | null>(null),
    mounted = useRef(true)
  useEffect(() => {
    let disposed = false,
      off: (() => void) | undefined
    mounted.current = true
    const refresh = () =>
      void meetingCall<Reply | null>('captions_reply_view')
        .then((v) => {
          if (!disposed) {
            if ((v?.noteMeetingId || undefined) !== meetingId) return
            setView(v)
            if (
              v &&
              ['processing', 'translating', 'ready', 'failed', 'cancelled'].includes(v.phase)
            ) {
              down.current = false
              setHolding(false)
            }
            if (v?.phase === 'ready') {
              if (meetingId && saved.current !== v.id) {
                saved.current = v.id
                savedCallback.current?.()
              }
              setText(v.text)
              setCopied(false)
            }
          }
        })
        .catch(() => {})
    refresh()
    if (!isTauri()) window.addEventListener('caption_reply_preview_changed', refresh)
    if (isTauri())
      void listen('caption_reply_changed', refresh).then((v) => {
        if (disposed) v()
        else off = v
      })
    const blur = () => {
      down.current = false
      if (id.current)
        void meetingCall('captions_reply_control', { id: id.current, action: 'release' }).catch(
          () => {},
        )
      setHolding(false)
    }
    window.addEventListener('blur', blur)
    return () => {
      disposed = true
      mounted.current = false
      off?.()
      window.removeEventListener('caption_reply_preview_changed', refresh)
      window.removeEventListener('blur', blur)
      if (down.current && id.current)
        void meetingCall('captions_reply_control', { id: id.current, action: 'cancel' }).catch(
          () => {},
        )
      down.current = false
    }
  }, [meetingId])
  const busy =
    starting ||
    (!!view && ['preparing', 'recording', 'processing', 'translating'].includes(view.phase))
  const start = async () => {
    if (busy || down.current) return
    down.current = true
    setHolding(true)
    setStarting(true)
    setError('')
    id.current = null
    try {
      const token = await meetingCall<string>('captions_reply_start', {
        model,
        targetLanguage: language,
        style,
        noteMeetingId: meetingId || null,
      })
      id.current = token
      if (!down.current || !mounted.current)
        await meetingCall('captions_reply_control', {
          id: token,
          action: mounted.current ? 'release' : 'cancel',
        })
    } catch (e) {
      if (mounted.current) {
        setError(meetingError(e))
        setHolding(false)
      }
      down.current = false
    } finally {
      if (mounted.current) setStarting(false)
    }
  }
  const release = () => {
    down.current = false
    setHolding(false)
    if (id.current)
      void meetingCall('captions_reply_control', { id: id.current, action: 'release' }).catch((e) =>
        setError(meetingError(e)),
      )
  }
  return (
    <details className="my-5 rounded-lg border p-4">
      <summary className="cursor-pointer text-sm font-medium">
        {meetingId
          ? t('口述个人补充', 'Speak a personal addition')
          : t('我来说 · 短句表达', 'My turn · short replies')}
      </summary>
      <p className="mt-3 text-xs leading-6 text-muted-foreground">
        {meetingId
          ? t(
              '按住说完后松开，以个人补充保存。最多 30 秒；不会当作会议事实。',
              'Hold to speak, then release to save a personal addition. Up to 30 seconds; it remains separate from meeting facts.',
            )
          : t(
              '按住说话，松开生成译文。最多 30 秒；不自动发送，不加入会议事实。',
              'Hold to speak, release for a translation. Up to 30 seconds. Nothing is sent automatically or added as a meeting fact.',
            )}
      </p>
      <div className="my-3 flex flex-wrap gap-3">
        {!meetingId && (
          <Select value={style} onValueChange={setStyle} disabled={busy}>
            <SelectTrigger aria-label={t('短句表达风格', 'Reply style')} className="w-36">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="concise">{t('简洁', 'Concise')}</SelectItem>
              <SelectItem value="polite">{t('礼貌', 'Polite')}</SelectItem>
              <SelectItem value="formal">{t('正式', 'Formal')}</SelectItem>
            </SelectContent>
          </Select>
        )}
        <Button
          disabled={busy && !holding}
          onPointerDown={(e) => {
            e.preventDefault()
            e.currentTarget.setPointerCapture(e.pointerId)
            void start()
          }}
          onPointerUp={release}
          onPointerCancel={release}
          onKeyDown={(e) => {
            if ((e.key === ' ' || e.key === 'Enter') && !e.repeat) {
              e.preventDefault()
              void start()
            }
          }}
          onKeyUp={(e) => {
            if (e.key === ' ' || e.key === 'Enter') {
              e.preventDefault()
              release()
            }
          }}
        >
          {starting || view?.phase === 'preparing'
            ? t('正在准备收音…', 'Preparing microphone…')
            : view?.phase === 'processing'
              ? t('正在整理短句…', 'Processing your sentence…')
              : holding
                ? meetingId
                  ? t('正在收音 · 松开保存', 'Listening · release to save')
                  : t('正在收音 · 松开出译文', 'Listening · release to translate')
                : view?.phase === 'translating'
                  ? t('正在生成译文…', 'Translating…')
                  : t('按住说一句', 'Hold to say a sentence')}
        </Button>
        {busy && (
          <Button
            variant="ghost"
            onClick={() => {
              down.current = false
              setHolding(false)
              if (id.current)
                void meetingCall('captions_reply_control', {
                  id: id.current,
                  action: 'cancel',
                }).catch((e) => setError(meetingError(e)))
            }}
          >
            {t('取消这句', 'Cancel this reply')}
          </Button>
        )}
      </div>
      {view?.original && (
        <p className="my-3 whitespace-pre-wrap text-sm leading-6">
          <span className="text-muted-foreground">{t('我说的：', 'Original: ')}</span>
          {view.original}
        </p>
      )}
      {view?.phase === 'ready' && (
        <>
          <Textarea
            aria-label={
              meetingId
                ? t('已保存的个人补充', 'Saved personal addition')
                : t('短句译文', 'Reply translation')
            }
            readOnly={!!meetingId}
            value={text}
            onChange={(e) => {
              setText(e.target.value)
              setCopied(false)
            }}
            className="min-h-20"
          />
          <Button
            className="mt-3"
            variant="outline"
            size="sm"
            disabled={!text.trim()}
            onClick={() =>
              void meetingCall('captions_copy', { text })
                .then(() => setCopied(true))
                .catch((e) => setError(meetingError(e)))
            }
          >
            {copied ? t('已复制', 'Copied') : t('复制这句', 'Copy reply')}
          </Button>
        </>
      )}
      {(error || view?.error) && (
        <p role="alert" className="mt-3 text-sm text-destructive">
          {error || view?.error}
        </p>
      )}
    </details>
  )
}
