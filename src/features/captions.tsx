import { ExperienceShortcuts, CaptionShortReply } from './caption-tools'
import { WorkspacePreferences } from './meeting-workspace'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from '@/components/ui/dialog'
import { meetingMessage } from './meeting-messages'
import { useText } from '@/lib/app-state'
import { Switch } from '@/components/ui/switch'
import { Collapsible, CollapsibleTrigger, CollapsibleContent } from '@/components/ui/collapsible'
import captionLanguages from '../../resources/caption-languages.json'
import { ConversationAssistant } from './conversation-assistant'
import { Slider } from '@/components/ui/slider'
import { Checkbox } from '@/components/ui/checkbox'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { TextServiceChoice } from './text-service-choice'
import { memo, useEffect, useMemo, useRef, useState } from 'react'
import { isTauri } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { Link, useLocation } from 'react-router-dom'
import {
  PiSubtitles,
  PiPlay,
  PiStop,
  PiPause,
  PiCopy,
  PiDownloadSimple,
  PiMicrophone,
  PiSpeakerHigh,
  PiCaretDown,
  PiEye,
} from 'react-icons/pi'
import { PageTitle, EmptyState } from '@/components/common'
import { Button } from '@/components/ui/button'
import { meetingCall, meetingError, timecode } from './meetings-api'
export interface CaptionLine {
  id: string
  startMs: number
  endMs: number
  original: string
  translated: string
  finalText: boolean
  translationWarning?: string | null
  source?: 'microphone' | 'system' | null
}
export interface CaptionView {
  translationEnabled: boolean
  translationStatus: string
  translationError: string | null
  displayHeld: boolean
  heldLines: CaptionLine[]
  uiLocale: string
  sessionId: string | null
  meetingId: string | null
  running: boolean
  paused: boolean
  mode: string
  targetLanguage: string
  error: string | null
  lines: CaptionLine[]
  save: boolean
  opacity: number
  fontSize: number
  original: boolean
  passthrough: boolean
  translationProviderId: string | null
  revision?: number
}
/** One changed line (or only translation status) on top of revision - 1. */
export interface CaptionLinePatch {
  revision: number
  line: CaptionLine | null
  translationStatus: string
  translationError: string | null
}
const captionLineLimit = 900
const lineOrder = (a: CaptionLine, b: CaptionLine) =>
  a.startMs - b.startMs || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0)
// Mirrors the native upsert so both sides keep the same ordered, bounded list.
export function upsertCaptionLine(lines: CaptionLine[], line: CaptionLine) {
  const next = lines.slice()
  let index = next.length - 1
  while (index >= 0 && next[index].id !== line.id) index--
  if (index >= 0) {
    next[index] = line
    if (
      (index > 0 && lineOrder(next[index - 1], line) > 0) ||
      (index < next.length - 1 && lineOrder(line, next[index + 1]) > 0)
    )
      next.sort(lineOrder)
  } else {
    let at = next.length
    while (at > 0 && lineOrder(next[at - 1], line) > 0) at--
    next.splice(at, 0, line)
  }
  if (next.length > captionLineLimit) next.shift()
  return next
}
export function applyCaptionPatch(view: CaptionView, patch: CaptionLinePatch): CaptionView {
  return {
    ...view,
    lines: patch.line ? upsertCaptionLine(view.lines, patch.line) : view.lines,
    translationStatus: patch.translationStatus,
    translationError: patch.translationError,
    revision: patch.revision,
  }
}
// Keeps one caption view current from full reads, whole-view events and line patches.
// Partial-result bursts coalesce into one update; a missed revision reloads the full view.
// Patches are never dropped: those not yet applicable wait for the next read, and run on top
// of it (older ones are skipped). At most one read is in flight, plus one trailing read.
export function captionSync(
  fetchView: () => Promise<CaptionView>,
  onView: (view: CaptionView) => void,
  onError: (error: unknown) => void,
  delayMs = 50,
) {
  let alive = true,
    current: CaptionView | null = null,
    queued: CaptionLinePatch[] = [],
    timer: ReturnType<typeof setTimeout> | undefined,
    reading = false,
    readAgain = false
  const commit = (next: CaptionView) => {
    if (current && (next.revision ?? 0) < (current.revision ?? 0)) return
    current = next
    onView(next)
  }
  const schedule = () => {
    timer ??= setTimeout(flush, delayMs)
  }
  const reload = () => {
    if (!alive) return
    if (reading) {
      readAgain = true
      return
    }
    reading = true
    readAgain = false
    fetchView().then(
      (view) => {
        reading = false
        if (!alive) return
        commit(view)
        if (readAgain) reload()
        flush()
      },
      (e) => {
        reading = false
        if (!alive) return
        onError(e)
        if (readAgain) reload()
      },
    )
  }
  function flush() {
    clearTimeout(timer)
    timer = undefined
    if (!alive) return
    // Without a base view the patches wait for the read that provides one.
    if (!current) {
      if (!reading) reload()
      return
    }
    const patches = queued.sort((a, b) => a.revision - b.revision)
    let next = current,
      applied = 0
    for (; applied < patches.length; applied++) {
      const patch = patches[applied],
        revision = next.revision ?? 0
      if (patch.revision <= revision) continue
      if (patch.revision !== revision + 1) break
      next = applyCaptionPatch(next, patch)
    }
    queued = patches.slice(applied)
    if (next !== current) commit(next)
    // A read in flight flushes again when it lands; otherwise fetch the missing revisions.
    if (queued.length && !reading) reload()
  }
  return {
    reload,
    view(view: CaptionView) {
      if (!alive) return
      commit(view)
      if (queued.length) schedule()
    },
    patch(patch: CaptionLinePatch) {
      if (!alive) return
      queued.push(patch)
      schedule()
    },
    dispose() {
      alive = false
      clearTimeout(timer)
    },
  }
}
export function useCaptions() {
  const [view, setView] = useState<CaptionView | null>(null),
    [error, setError] = useState('')
  useEffect(() => {
    let alive = true
    const sync = captionSync(
      () => meetingCall<CaptionView>('captions_view'),
      setView,
      (e) => setError(meetingError(e)),
    )
    const subscriptions: (() => void)[] = [sync.dispose]
    sync.reload()
    if (!isTauri()) {
      const refresh = () => sync.reload()
      window.addEventListener('captions_preview_changed', refresh)
      subscriptions.push(() => window.removeEventListener('captions_preview_changed', refresh))
    }
    if (isTauri()) {
      const subscribe = (promise: Promise<() => void>) =>
        void promise
          .then((off) => {
            if (alive) subscriptions.push(off)
            else off()
          })
          .catch((e) => {
            if (alive) setError(meetingError(e))
          })
      subscribe(listen<CaptionView>('captions_changed', (e) => sync.view(e.payload)))
      subscribe(listen<CaptionLinePatch>('captions_line', (e) => sync.patch(e.payload)))
      subscribe(listen('settings_changed', () => sync.reload()))
    }
    return () => {
      alive = false
      subscriptions.forEach((off) => off())
    }
  }, [])
  return { view, error, setError }
}
const field = 'rounded-md border bg-background px-3 py-2 text-sm'
type Appearance = Pick<CaptionView, 'opacity' | 'fontSize' | 'original' | 'passthrough'>
const CaptionRow = memo(function CaptionRow({
  line: l,
  translationEnabled,
  fontSize,
  retrying,
  t,
}: {
  line: CaptionLine
  translationEnabled: boolean
  fontSize: number
  retrying: boolean
  t: (zh: string, en: string) => string
}) {
  return (
    <div className="grid grid-cols-[48px_1fr] gap-5">
      <span className="text-xs text-muted-foreground tabular-nums pt-1">{timecode(l.startMs)}</span>
      <div>
        {l.source && (
          <p className="mb-1 text-[11px] text-muted-foreground">
            {l.source === 'microphone' ? t('麦克风', 'Microphone') : t('系统声音', 'System audio')}
          </p>
        )}
        <p
          className={translationEnabled ? 'text-muted-foreground' : ''}
          style={{
            fontSize: translationEnabled ? Math.max(14, fontSize * 0.75) : fontSize,
            lineHeight: 1.6,
          }}
        >
          {l.original}
          {!l.finalText && <span className="ml-2 text-[10px]">{t('识别中', 'Recognizing')}</span>}
        </p>
        {translationEnabled && (
          <p className="mt-1 min-h-8" style={{ fontSize, lineHeight: 1.6 }}>
            {l.translated ||
              (l.finalText
                ? retrying
                  ? t(
                      '翻译暂不可用，原文仍在更新',
                      'Translation unavailable; original captions continue',
                    )
                  : t('正在翻译…', 'Translating…')
                : '')}
          </p>
        )}
        {l.translationWarning && (
          <p className="mt-1 text-xs text-muted-foreground">
            {t(
              '译文数字可能不一致，请核对上方原文。',
              'Translated numbers may differ. Check the original above.',
            )}
          </p>
        )}
      </div>
    </div>
  )
})
export function Captions() {
  const t = useText()
  const location = useLocation()

  const { view, error, setError } = useCaptions(),
    [mode, setMode] = useState('listen'),
    [model, setModel] = useState('zipformer'),
    [targetLanguage, setTargetLanguage] = useState('zh-CN'),
    [save, setSave] = useState(false),
    [pending, setPending] = useState(false),
    [draft, setDraft] = useState(''),
    [edited, setEdited] = useState(false)
  const [apps, setApps] = useState<{ pid: number; name: string }[]>([]),
    [appPickerOpen, setAppPickerOpen] = useState(false),
    [pid, setPid] = useState<number | null>(null)
  const [advancedOpen, setAdvancedOpen] = useState(false)
  const [translationEnabled, setTranslationEnabled] = useState(false)
  const [presetFeedback, setPresetFeedback] = useState<{
    fontSize: number
    unchanged: boolean
  } | null>(null)
  const [recap, setRecap] = useState<{
    text: string
    lines: CaptionLine[]
    generated: boolean
  } | null>(null)
  const [recapSeconds, setRecapSeconds] = useState('60')
  const [assistantOpen, setAssistantOpen] = useState(false)
  const [following, setFollowing] = useState(true)
  const history = useRef<HTMLDivElement>(null),
    historyContent = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (view) setTranslationEnabled(view.translationEnabled)
  }, [view?.translationEnabled])
  const shownLines = view ? (view.displayHeld ? view.heldLines : view.lines) : []
  const followLatest = following && !view?.displayHeld,
    hasLines = !!view?.lines.length
  // Follow whenever the content grows (new lines, late translations, a larger font or the
  // translation row appearing), not on every unrelated update.
  useEffect(() => {
    const el = history.current,
      content = historyContent.current
    if (!followLatest || !el) return
    const toLatest = () => {
      el.scrollTop = el.scrollHeight
    }
    toLatest()
    if (!content || typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver(toLatest)
    observer.observe(content)
    return () => observer.disconnect()
  }, [followLatest, hasLines])
  const systemEnabled = mode === 'listen' || mode === 'mixed'
  const micEnabled = mode === 'speak' || mode === 'mixed'
  const setSource = (source: 'system' | 'microphone', enabled: boolean) => {
    const system = source === 'system' ? enabled : systemEnabled
    const microphone = source === 'microphone' ? enabled : micEnabled
    setMode(system ? (microphone ? 'mixed' : 'listen') : microphone ? 'speak' : 'none')
  }
  const sourceLabel =
    mode === 'mixed'
      ? t('系统声音 + 麦克风', 'System audio + microphone')
      : mode === 'listen'
        ? t('系统声音', 'System audio')
        : mode === 'speak'
          ? t('麦克风', 'Microphone')
          : t('未选择音源', 'No audio source selected')
  const act = async (fn: () => Promise<unknown>) => {
    setPending(true)
    setError('')
    try {
      await fn()
    } catch (e) {
      setError(meetingError(e))
    } finally {
      setPending(false)
    }
  }
  const [archives, setArchives] = useState<{ id: string; at: number }[]>([])
  const [archiveId, setArchiveId] = useState('')
  const archiveRequest = useRef('')
  useEffect(() => {
    const id = new URLSearchParams(location.search).get('archive')
    if (!view || !id || !/^[0-9a-f-]{36}$/i.test(id)) return
    const request = `${location.key}:${id}`
    if (archiveRequest.current === request) return
    archiveRequest.current = request
    if (view.running) {
      setError(
        t(
          '字幕正在运行，请结束后再打开已保存的记录。',
          'Captions are running. Finish them before opening a saved record.',
        ),
      )
      return
    }
    let live = true
    setPending(true)
    void meetingCall('captions_archive_open', { id })
      .then(() => {
        if (live) setArchiveId(id)
      })
      .catch((failure) => {
        if (live) setError(meetingError(failure))
      })
      .finally(() => {
        if (live) setPending(false)
      })
    return () => {
      live = false
    }
  }, [location.key, location.search, !!view, t])
  useEffect(() => {
    if (isTauri())
      void meetingCall<typeof archives>('captions_archives')
        .then(setArchives)
        .catch(() => {})
  }, [view?.running])
  useEffect(() => {
    if (view?.mode) setMode(view.mode)
  }, [view?.mode])
  useEffect(() => {
    if (view?.targetLanguage) setTargetLanguage(view.targetLanguage)
  }, [view?.targetLanguage])
  const showDraft = mode === 'speak' && !view?.running && !!view?.lines.length
  const viewLines = view?.lines
  const allText = useMemo(
    () =>
      showDraft && viewLines
        ? viewLines
            .map((l) => (translationEnabled ? l.translated : l.original))
            .filter(Boolean)
            .join(' ')
        : '',
    [showDraft, viewLines, translationEnabled],
  )
  const draftText = edited ? draft : allText
  const [opacityDraft, setOpacityDraft] = useState<number | null>(null)
  const viewOpacity = Math.round((view?.opacity ?? 0.92) * 100)
  const dragging = useRef(false)
  useEffect(() => {
    if (!dragging.current && opacityDraft === viewOpacity) setOpacityDraft(null)
  }, [viewOpacity, opacityDraft])
  // Radix commits only a changed value, so a drag released where it started ends here too.
  const endOpacityDrag = () => {
    if (!dragging.current) return
    dragging.current = false
    setOpacityDraft((draft) => (draft === viewOpacity ? null : draft))
  }
  const latestView = useRef(view)
  latestView.current = view
  // One request in flight; later edits (presets included) merge onto the newest requested values
  // instead of a stale view. Resolves true once the change is applied, false if it failed.
  const appearanceQueue = useRef<{
    busy: boolean
    sent: Appearance | null
    next: Appearance | null
    waiting: ((applied: boolean) => void)[]
  }>({ busy: false, sent: null, next: null, waiting: [] })
  const appearance = (patch: Partial<Appearance>) =>
    new Promise<boolean>((resolve) => {
      setPresetFeedback(null)
      const current = latestView.current
      if (!current) return resolve(false)
      const queue = appearanceQueue.current
      queue.next = {
        ...(queue.next ??
          queue.sent ?? {
            opacity: current.opacity,
            fontSize: current.fontSize,
            original: current.original,
            passthrough: current.passthrough,
          }),
        ...patch,
      }
      queue.waiting.push(resolve)
      if (queue.busy) return
      const send = () => {
        const request = queue.next,
          waiting = queue.waiting
        queue.next = null
        queue.waiting = []
        queue.sent = request
        queue.busy = !!request
        if (!request) return
        void meetingCall('captions_appearance', { ...request })
          .then(
            () => {
              setError('')
              waiting.forEach((done) => done(true))
            },
            (e) => {
              setError(meetingError(e))
              setOpacityDraft(null)
              waiting.forEach((done) => done(false))
            },
          )
          .finally(send)
      }
      send()
    })
  const readingFontSize = view?.fontSize ?? 24
  const activePresetFont =
    view && !translationEnabled && view.original && !view.passthrough ? readingFontSize : null
  return (
    <>
      <PageTitle
        title={t('实时字幕', 'Live captions')}
        description={t(
          '视频、课程、播客和实时对话，都能边听边看。',
          'Follow videos, lessons, podcasts and live conversations as you listen.',
        )}
        action={
          view?.running ? (
            <div className="flex items-center gap-2">
              <Button
                variant="outline"
                disabled={pending}
                onClick={() => void act(() => meetingCall('captions_window', { action: 'show' }))}
              >
                <PiEye />
                {t('显示字幕', 'Show captions')}
              </Button>
              <Button
                variant="outline"
                disabled={pending}
                onClick={() =>
                  void act(() =>
                    meetingCall('captions_control', { action: view.paused ? 'resume' : 'pause' }),
                  )
                }
              >
                {view.paused ? <PiPlay /> : <PiPause />}
                {view.paused ? t('继续', 'Resume') : t('暂停', 'Pause')}
              </Button>
              <Button
                disabled={pending}
                onClick={() => void act(() => meetingCall('captions_control', { action: 'stop' }))}
              >
                <PiStop />
                {t('结束字幕', 'Stop captions')}
              </Button>
            </div>
          ) : (
            <Button
              disabled={pending || (!systemEnabled && !micEnabled)}
              onClick={() =>
                void act(async () => {
                  setEdited(false)
                  await meetingCall('captions_start', {
                    mode,
                    model,
                    appPid: systemEnabled ? pid : null,
                    save,
                    targetLanguage,
                    translationEnabled,
                  })
                })
              }
            >
              <PiPlay />
              {t('开始字幕', 'Start captions')}
            </Button>
          )
        }
      />
      {!view?.running && archives.length > 0 && (
        <div className="mb-6 flex items-center gap-2">
          <Select
            value={archiveId}
            disabled={pending}
            onValueChange={(value) => {
              setArchiveId(value)
              void act(() => meetingCall('captions_archive_open', { id: value }))
            }}
          >
            <SelectTrigger aria-label={t('已保存的字幕', 'Saved captions')} className="w-[240px]">
              <SelectValue placeholder={t('查看已保存的字幕', 'View saved captions')} />
            </SelectTrigger>
            <SelectContent>
              {archives.map((a) => (
                <SelectItem key={a.id} value={a.id}>
                  {new Date(a.at).toLocaleString(t('zh-CN', 'en'))}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {archiveId && (
            <Button
              variant="ghost"
              disabled={pending}
              onClick={() =>
                void act(async () => {
                  await meetingCall('captions_archive_delete', { id: archiveId })
                  setArchives((a) => a.filter((i) => i.id !== archiveId))
                  setArchiveId('')
                })
              }
            >
              {t('删除保存记录', 'Delete saved captions')}
            </Button>
          )}
        </div>
      )}
      <section className="mb-5 space-y-3" aria-label={t('字幕阅读预设', 'Caption reading presets')}>
        <div className="flex flex-wrap items-center gap-2">
          <span className="mr-1 text-xs text-muted-foreground">
            {t('阅读预设', 'Reading presets')}
          </span>
          {[
            [24, t('标准原文', 'Standard original text')],
            [32, t('大字原文', 'Large original text')],
          ].map(([font, label]) => (
            <Button
              key={font}
              variant={activePresetFont === font ? 'default' : 'outline'}
              size="sm"
              aria-pressed={activePresetFont === font}
              disabled={pending || !view}
              onClick={() => {
                setPresetFeedback(null)
                const fontSize = Number(font)
                const unchanged = activePresetFont === fontSize
                if (unchanged) {
                  setPresetFeedback({ fontSize, unchanged })
                  return
                }
                void act(async () => {
                  await meetingCall('captions_translation', { enabled: false })
                  setTranslationEnabled(false)
                  // Through the shared queue, so pending slider or font edits cannot revert it.
                  if (await appearance({ fontSize, original: true, passthrough: false }))
                    setPresetFeedback({ fontSize, unchanged })
                })
              }}
            >
              {label}
            </Button>
          ))}
        </div>
        <p className="text-xs leading-6 text-muted-foreground">
          {t(
            '标准原文 24 号字，大字原文 32 号字；关闭翻译，方便阅读和复制。字号同步到此页面和悬浮字幕。',
            'Standard uses 24 px; large uses 32 px. Both turn off translation and allow reading and copying. Text size applies here and in floating captions.',
          )}
        </p>
        <div className="rounded-lg border bg-muted/20 px-4 py-3">
          <p className="mb-2 text-[11px] text-muted-foreground">
            {t('阅读效果预览', 'Reading preview')}
          </p>
          <p style={{ fontSize: readingFontSize, lineHeight: 1.6 }}>
            {t(
              '这是一行字幕，看看是否读得清楚。',
              'A sample caption. Check that it is easy to read.',
            )}
          </p>
        </div>
        {presetFeedback && activePresetFont === presetFeedback.fontSize && (
          <p role="status" className="text-xs leading-6 text-muted-foreground">
            {presetFeedback.unchanged
              ? t('当前已经是此设置：', 'Already using these settings: ')
              : t('已应用：', 'Applied: ')}
            {t(
              `${presetFeedback.fontSize} 号字、仅原文。`,
              `${presetFeedback.fontSize} px, original text only. `,
            )}
            {!view?.running &&
              t(
                '点击“开始字幕”后使用此设置。',
                'Choose Start captions to begin listening with these settings.',
              )}
          </p>
        )}
      </section>
      <div className="mb-5 flex flex-wrap items-center justify-between gap-3">
        <label className="flex items-center gap-3 text-sm">
          <Switch
            checked={translationEnabled}
            disabled={pending}
            onCheckedChange={(enabled) => {
              setPresetFeedback(null)
              if (view?.running) void act(() => meetingCall('captions_translation', { enabled }))
              else setTranslationEnabled(enabled)
            }}
            aria-label={t('翻译字幕', 'Translate captions')}
          />
          <span>
            {translationEnabled
              ? t('原文 + 翻译', 'Original + translation')
              : t('仅原文字幕', 'Original captions only')}
          </span>
        </label>
        <WorkspacePreferences termsOnly />
      </div>
      {!translationEnabled && (
        <p className="mb-5 text-xs text-muted-foreground">
          {t(
            '仅在本机识别，不调用翻译服务；无需文字模型。',
            'Recognizes speech on your device without translation calls or a text model.',
          )}
        </p>
      )}
      {translationEnabled && (
        <TextServiceChoice
          purpose="captions"
          disabled={pending}
          onSelectionChange={() => {
            if (view?.running)
              void act(() => meetingCall('captions_translation', { enabled: true }))
          }}
        />
      )}
      <section className="mb-6" aria-label={t('字幕音频来源', 'Caption audio sources')}>
        <div className="mb-3 flex items-baseline gap-3">
          <h2 className="text-sm font-medium">{t('采集声音', 'Audio sources')}</h2>
          <span className="text-xs text-muted-foreground">
            {t('两项可同时开启', 'Both can be enabled together')}
          </span>
        </div>
        <div className="divide-y rounded-lg border px-4">
          <div className="flex items-center gap-3 py-4">
            <PiSpeakerHigh aria-hidden className="size-5 shrink-0 text-muted-foreground" />
            <label htmlFor="caption-system" className="flex-1 cursor-pointer">
              <span className="block text-sm font-medium">{t('系统声音', 'System audio')}</span>
              <span className="mt-1 block text-xs leading-5 text-muted-foreground">
                {pid
                  ? t(
                      `仅采集 ${apps.find((app) => app.pid === pid)?.name ?? '已选应用'} 的声音`,
                      `Capture audio only from ${apps.find((app) => app.pid === pid)?.name ?? 'the selected app'}`,
                    )
                  : t(
                      '采集会议、视频等电脑播放的声音，无需选择应用',
                      'Capture audio from meetings, videos and other apps. No app selection needed.',
                    )}
              </span>
            </label>
            <Switch
              id="caption-system"
              aria-label={t('系统声音', 'System audio')}
              checked={systemEnabled}
              disabled={view?.running || pending}
              onCheckedChange={(checked) => setSource('system', checked)}
            />
          </div>
          <div className="flex items-center gap-3 py-4">
            <PiMicrophone aria-hidden className="size-5 shrink-0 text-muted-foreground" />
            <label htmlFor="caption-microphone" className="flex-1 cursor-pointer">
              <span className="block text-sm font-medium">{t('麦克风', 'Microphone')}</span>
              <span className="mt-1 block text-xs leading-5 text-muted-foreground">
                {t('同时记录你说的话', 'Include your voice too')}
              </span>
            </label>
            <Switch
              id="caption-microphone"
              aria-label={t('麦克风', 'Microphone')}
              checked={micEnabled}
              disabled={view?.running || pending}
              onCheckedChange={(checked) => setSource('microphone', checked)}
            />
          </div>
        </div>
        {!systemEnabled && !micEnabled && (
          <p role="status" className="mt-2 text-xs text-muted-foreground">
            {t('请至少开启一个音频来源。', 'Enable at least one audio source.')}
          </p>
        )}
        {systemEnabled && micEnabled && (
          <p className="mt-2 text-xs text-muted-foreground">
            {t(
              '建议佩戴耳机，避免麦克风再次录入扬声器中的声音。',
              'Use headphones to keep the microphone from picking up speaker audio again.',
            )}
          </p>
        )}
        <Collapsible open={advancedOpen} onOpenChange={setAdvancedOpen} className="mt-2">
          <CollapsibleTrigger asChild>
            <Button variant="ghost" size="sm" className="-ml-2 gap-1.5 text-muted-foreground">
              {t('高级选项', 'Advanced options')}
              <PiCaretDown
                className={
                  advancedOpen ? 'rotate-180 transition-transform' : 'transition-transform'
                }
              />
            </Button>
          </CollapsibleTrigger>
          <CollapsibleContent className="pt-3">
            <div className="flex flex-wrap items-center gap-3">
              <label htmlFor="caption-application" className="text-xs text-muted-foreground">
                {t('仅采集指定应用（可选）', 'Capture a specific app only (optional)')}
              </label>
              <Select
                disabled={!systemEnabled || view?.running || pending}
                value={String(pid ?? 0)}
                open={appPickerOpen}
                onOpenChange={(open) => {
                  if (!open) {
                    setAppPickerOpen(false)
                    return
                  }
                  void act(async () => {
                    setApps(await meetingCall('meeting_audio_apps'))
                    setAppPickerOpen(true)
                  })
                }}
                onValueChange={(value) => setPid(Number(value) || null)}
              >
                <SelectTrigger id="caption-application" className="w-[220px]">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="0">
                    {t('全部系统声音（默认）', 'All system audio (default)')}
                  </SelectItem>
                  {pid && !apps.some((app) => app.pid === pid) && (
                    <SelectItem value={String(pid)}>
                      {t('已选应用（可能已退出）', 'Selected app (may have quit)')}
                    </SelectItem>
                  )}
                  {apps.map((app) => (
                    <SelectItem key={app.pid} value={String(app.pid)}>
                      {app.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {!!pid && (
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={view?.running || pending}
                  onClick={() => setPid(null)}
                >
                  {t('恢复全部系统声音', 'Use all system audio')}
                </Button>
              )}
            </div>
          </CollapsibleContent>
        </Collapsible>
      </section>
      <div className="mb-6 grid gap-5 sm:grid-cols-2">
        <div>
          <label htmlFor="caption-target" className="mb-3 block text-sm font-medium">
            {t('翻译为', 'Translate to')}
          </label>
          <Select
            value={targetLanguage}
            onValueChange={setTargetLanguage}
            disabled={!translationEnabled || view?.running || pending}
          >
            <SelectTrigger id="caption-target">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {captionLanguages.map((language) => (
                <SelectItem key={language.code} value={language.code}>
                  {t(language.name, language.englishName)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div>
          <label htmlFor="caption-model" className="mb-3 block text-sm font-medium">
            {t('识别模型', 'Speech recognition model')}
          </label>
          <Select disabled={view?.running || pending} value={model} onValueChange={setModel}>
            <SelectTrigger
              id="caption-model"
              aria-label={t('字幕识别模型', 'Caption speech model')}
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="zipformer">
                {t('Zipformer · 中英识别', 'Zipformer · Chinese and English')}
              </SelectItem>
              <SelectItem value="zipformer-en">
                {t('Zipformer · 英语轻量', 'Zipformer · Lightweight English')}
              </SelectItem>
              <SelectItem value="paraformer">
                {t('Paraformer · 中英识别', 'Paraformer · Chinese and English')}
              </SelectItem>
            </SelectContent>
          </Select>
        </div>
      </div>
      <div className="mb-6 flex flex-wrap items-center justify-between gap-3">
        <p className="text-xs leading-6 text-muted-foreground">
          {model === 'zipformer-en'
            ? t(
                '此模型仅识别英语。需要识别中文时请选择中英模型。',
                'This model recognizes English only. Choose a bilingual model for Chinese speech.',
              )
            : t(
                '当前模型自动识别中英文，直接显示原文。开启翻译后使用所选目标语言。',
                'This model detects Chinese and English and displays the original speech. Enable translation to use your target language.',
              )}
        </p>
        <label className="flex items-center gap-2 text-xs text-muted-foreground">
          <Checkbox
            disabled={view?.running}
            checked={save}
            onCheckedChange={(value) => setSave(value === true)}
          />
          {t('保存字幕文字', 'Save caption text')}
        </label>
      </div>
      {(error || view?.error) && (
        <p role="alert" className="mb-5 rounded-lg bg-muted px-4 py-3 text-sm">
          {meetingMessage(error || view?.error, t)}
        </p>
      )}
      {view?.translationError && translationEnabled && (
        <p role="status" className="mb-4 rounded-lg bg-muted px-4 py-3 text-sm">
          {meetingMessage(view.translationError, t)}
        </p>
      )}
      <div className="mb-3 flex flex-wrap items-center gap-2">
        <Button
          size="sm"
          variant="outline"
          disabled={pending || !view?.lines.length}
          onClick={() =>
            void act(() =>
              meetingCall('captions_control', { action: view?.displayHeld ? 'live' : 'hold' }),
            )
          }
        >
          {view?.displayHeld ? t('回到实时', 'Back to live') : t('暂留字幕', 'Hold captions')}
        </Button>
        <Button
          size="sm"
          variant="ghost"
          disabled={!view?.lines.length}
          onClick={() => {
            setFollowing(true)
            if (view?.displayHeld)
              void act(() => meetingCall('captions_control', { action: 'live' }))
          }}
        >
          {following ? t('跟随最新字幕', 'Following latest') : t('跳到最新', 'Jump to latest')}
        </Button>
        <Select value={recapSeconds} onValueChange={setRecapSeconds}>
          <SelectTrigger aria-label={t('回顾范围', 'Recap range')} className="ml-auto w-36">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {[30, 60, 120].map((seconds) => (
              <SelectItem key={seconds} value={String(seconds)}>
                {t(`最近 ${seconds} 秒`, `Last ${seconds} seconds`)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Button
          size="sm"
          disabled={pending || !view?.lines.length}
          onClick={() =>
            void act(async () =>
              setRecap(await meetingCall('captions_recap', { seconds: Number(recapSeconds) })),
            )
          }
        >
          {t('回顾刚才', 'Recap recent speech')}
        </Button>
      </div>
      {view?.displayHeld && (
        <p role="status" className="mb-3 text-xs text-muted-foreground">
          {t(
            '显示已暂留，识别与录音仍在继续。点击“回到实时”查看新内容。',
            'Display held. Recognition and recording continue. Choose Back to live to see new speech.',
          )}
        </p>
      )}
      <section className="min-h-[280px] rounded-xl border p-6">
        <div className="mb-5 flex justify-between text-xs text-muted-foreground">
          <span>
            {view?.running
              ? view.paused
                ? t('已暂停', 'Paused')
                : t('正在聆听', 'Listening')
              : t('字幕预览', 'Caption preview')}
          </span>
          <span>
            {sourceLabel}
            {t(' · 本机识别', ' · On-device recognition')}
          </span>
        </div>
        {view?.lines.length ? (
          <div
            ref={history}
            onScroll={() => {
              const el = history.current
              if (el) setFollowing(el.scrollHeight - el.scrollTop - el.clientHeight < 40)
            }}
            className="max-h-[380px] overflow-y-auto"
          >
            <div ref={historyContent} className="space-y-6">
              {shownLines.map((l) => (
                <CaptionRow
                  key={l.id}
                  line={l}
                  translationEnabled={translationEnabled}
                  fontSize={readingFontSize}
                  retrying={view.translationStatus === 'retrying'}
                  t={t}
                />
              ))}
            </div>
          </div>
        ) : (
          <EmptyState
            icon={PiSubtitles}
            title={
              view?.running
                ? t('正在准备识别', 'Preparing recognition')
                : t('让字幕跟上正在听的内容', 'Follow what you are listening to')
            }
            description={
              systemEnabled
                ? t(
                    translationEnabled
                      ? '启动后，所选音源可在悬浮窗中显示双语字幕。'
                      : '启动后，在悬浮窗中显示原文字幕。可随时开启翻译。',
                    translationEnabled
                      ? 'Start to see bilingual captions for your selected audio sources in a floating window.'
                      : 'Show original captions in a floating window. Translation can be enabled anytime.',
                  )
                : t(
                    '说完后查看、编辑译文，再复制或填入其他应用。',
                    'Review and edit your translation, then copy or insert it into another app.',
                  )
            }
          />
        )}
      </section>
      <ExperienceShortcuts t={t} />
      <CaptionShortReply t={t} model={model} language={targetLanguage} />
      <Dialog
        open={!!recap}
        onOpenChange={(open) => {
          if (!open) setRecap(null)
        }}
      >
        <DialogContent closeLabel={t('关闭', 'Close')}>
          <DialogHeader>
            <DialogTitle>{t('刚才说了什么', 'What was just said')}</DialogTitle>
            <DialogDescription>
              {recap?.generated
                ? t(
                    '依据最近原文整理，可展开原文核对。',
                    'Based on recent speech. Check the source below.',
                  )
                : t(
                    '当前未启用 AI 整理，直接回看最近的完整原文。',
                    'AI recap is unavailable. Review the complete recent transcript here.',
                  )}
            </DialogDescription>
          </DialogHeader>
          <p className="max-h-56 overflow-y-auto whitespace-pre-wrap text-sm leading-7">
            {recap?.text}
          </p>
          <details>
            <summary className="cursor-pointer text-xs">{t('查看原文', 'View source')}</summary>
            <div className="mt-3 max-h-48 overflow-y-auto space-y-2">
              {recap?.lines.map((l) => (
                <p className="text-sm leading-6" key={l.id}>
                  <span className="mr-3 text-xs text-muted-foreground">{timecode(l.startMs)}</span>
                  {l.original}
                </p>
              ))}
            </div>
          </details>
        </DialogContent>
      </Dialog>
      {showDraft && (
        <section className="mt-7">
          <h2 className="mb-3 text-sm font-medium">
            {translationEnabled
              ? t('确认译文', 'Review translation')
              : t('确认原文', 'Review original text')}
          </h2>
          <textarea
            aria-label={t('待发送的译文', 'Translation to send')}
            className={field + ' w-full min-h-32 leading-7'}
            maxLength={4000}
            value={draftText}
            onChange={(e) => {
              setEdited(true)
              setDraft(e.target.value)
            }}
          />
          <div className="mt-3 flex justify-end gap-2">
            <Button
              variant="outline"
              disabled={!draftText || pending}
              onClick={() => void act(() => meetingCall('captions_copy', { text: draftText }))}
            >
              <PiCopy />
              {translationEnabled
                ? t('复制译文', 'Copy translation')
                : t('复制原文', 'Copy original text')}
            </Button>
            <Button
              disabled={!draftText || pending || !!view?.running}
              onClick={() =>
                void act(async () => {
                  setError(
                    t(
                      '请在 10 秒内切换到目标应用并点击输入框。',
                      'Switch to the target app and click its text field within 10 seconds.',
                    ),
                  )
                  await meetingCall('captions_paste', { text: draftText })
                  setError(
                    t(
                      '已执行一次填入，请在目标应用确认。',
                      'Insertion attempted once. Check the result in the target app.',
                    ),
                  )
                })
              }
            >
              {t('选择输入框后填入', 'Insert after selecting a text field')}
            </Button>
          </div>
        </section>
      )}
      {view?.sessionId && (
        <div className="mt-10 border-t pt-8">
          <Button
            variant="outline"
            aria-expanded={assistantOpen}
            onClick={() => setAssistantOpen(!assistantOpen)}
          >
            {assistantOpen
              ? t('收起字幕助手', 'Hide caption assistant')
              : t('问问这段内容', 'Ask about this content')}
          </Button>
          <p className="mt-2 text-xs leading-6 text-muted-foreground">
            {view.meetingId
              ? t(
                  '当前字幕已关联一场会议，助手共用该会议的上下文。',
                  'These captions are linked to a meeting. The assistant shares its context.',
                )
              : t(
                  '独立字幕会话，可用于看视频、听课程和播客。',
                  'An independent caption session for videos, lessons and podcasts.',
                )}
          </p>
          {assistantOpen && (
            <div className="mt-6">
              {!translationEnabled && !view.meetingId && (
                <TextServiceChoice purpose="captions" disabled={pending} />
              )}
              <ConversationAssistant
                surface="captions"
                meetingId={view.meetingId ?? undefined}
                contextKey={view.sessionId + (view.meetingId ?? '')}
              />
            </div>
          )}
        </div>
      )}
      <div className="mt-7 flex flex-wrap gap-6 items-center border-t pt-5 text-xs text-muted-foreground">
        <label className="flex gap-2 items-center">
          {t('字号', 'Font size')}
          <Select
            value={String(view?.fontSize ?? 24)}
            onValueChange={(value) => appearance({ fontSize: Number(value) })}
          >
            <SelectTrigger aria-label={t('字幕字号', 'Caption font size')} className="w-[80px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {[18, 24, 30, 32, 36].map((n) => (
                <SelectItem key={n} value={String(n)}>
                  {n}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </label>
        <div className="flex items-center gap-3">
          <span>{t('透明度', 'Opacity')}</span>
          <Slider
            aria-label={t('字幕透明度', 'Caption opacity')}
            className="w-28"
            min={40}
            max={100}
            step={1}
            value={[opacityDraft ?? viewOpacity]}
            onValueChange={([value]) => {
              dragging.current = true
              setOpacityDraft(value)
              void appearance({ opacity: value / 100 })
            }}
            onValueCommit={endOpacityDrag}
            onPointerUp={endOpacityDrag}
            onLostPointerCapture={endOpacityDrag}
          />
          <span className="w-9 text-right tabular-nums">{opacityDraft ?? viewOpacity}%</span>
        </div>
        <label className="flex items-center gap-2">
          <Checkbox
            checked={view?.original ?? true}
            onCheckedChange={(value) => appearance({ original: value === true })}
          />
          {t('显示原文', 'Show original text')}
        </label>
        <label className="flex items-center gap-2">
          <Checkbox
            checked={view?.passthrough ?? false}
            onCheckedChange={(value) => appearance({ passthrough: value === true })}
          />
          {t('鼠标穿透', 'Click-through')}
        </label>
        <Select
          value=""
          disabled={!view?.lines.length || pending}
          onValueChange={(value) =>
            void act(() => meetingCall('captions_export', { format: value }))
          }
        >
          <SelectTrigger
            aria-label={t('导出字幕', 'Export captions')}
            className="ml-auto w-[140px]"
          >
            <SelectValue placeholder={t('导出字幕', 'Export captions')} />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="txt">TXT</SelectItem>
            <SelectItem value="srt">SRT</SelectItem>
            <SelectItem value="vtt">VTT</SelectItem>
          </SelectContent>
        </Select>
      </div>
      <p className="mt-5 text-xs leading-6 text-muted-foreground">
        {systemEnabled
          ? t(
              '系统声音需要屏幕与系统音频录制权限。',
              'System audio requires Screen & System Audio Recording permission.',
            )
          : ''}
        {micEnabled
          ? t('麦克风需要录音权限。', 'Microphone access requires recording permission.')
          : ''}
        <Link to="/settings/models" className="ml-2 underline underline-offset-4">
          {t('管理本地模型', 'Manage local models')}
        </Link>
      </p>
    </>
  )
}
