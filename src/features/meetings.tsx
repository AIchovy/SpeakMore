import {
  WorkspacePreferences,
  TemplateChoice,
  PersonalResult,
  SpeakerNames,
  FullTextResults,
  AcrossMeetingTasks,
  type Preferences,
} from './meeting-workspace'
import { MeetingWorkbench, ItemState, PreparationCard, LibraryResearch } from './meeting-polish'
import { meetingMessage } from './meeting-messages'
import { MeetingMicrophoneAccess } from './meeting-microphone-access'
import { MeetingAudio } from './meeting-audio'
import { MeetingItemReview } from './meeting-item-review'
import { useText } from '@/lib/app-state'
import { ConversationAssistant } from './conversation-assistant'
import { Checkbox } from '@/components/ui/checkbox'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { TextServiceChoice } from './text-service-choice'
import { MossSetup } from './moss-setup'
import { LiveRecordingTime } from './meeting-activity'
import { MeetingExtras, AudioSource } from './meeting-extras'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { isTauri } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { Link, useLocation, useSearchParams } from 'react-router-dom'
import { toast } from 'sonner'
import {
  PiArrowLeft,
  PiCheck,
  PiCopy,
  PiDownloadSimple,
  PiMicrophone,
  PiNotepad,
  PiPause,
  PiPencilSimple,
  PiPlay,
  PiPlus,
  PiStop,
  PiTrash,
  PiUploadSimple,
  PiX,
} from 'react-icons/pi'
import { PageTitle, EmptyState } from '@/components/common'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from '@/components/ui/dialog'
import { Tabs, TabsList, TabsTrigger, TabsContent } from '@/components/ui/tabs'
import {
  meetingCall,
  meetingError,
  meetingKey,
  meetingListKey,
  meetingMissingMessage,
  meetingStatus,
  meetingStatusLabel,
  timecode,
  type Meeting,
  type MeetingItem,
  type MeetingList,
  type MeetingSegment,
  type ImportChoice,
} from './meetings-api'

const fieldClass =
  'w-full rounded-md border bg-background px-3 py-2 text-sm focus-visible:outline-2 focus-visible:outline-ring'

export function Meetings() {
  const t = useText()
  const location = useLocation()
  const [, setSearchParams] = useSearchParams()

  const groups: [MeetingItem['kind'], string][] = [
    ['decision', t('决策事项', 'Decisions')],
    ['todo', t('待办清单', 'Action items')],
    ['fact', t('关键数字与结论', 'Key figures and findings')],
    ['next', t('下一步行动', 'Next steps')],
  ]

  const [project, setProject] = useState(''),
    [filterProject, setFilterProject] = useState(''),
    [after, setAfter] = useState(''),
    [before, setBefore] = useState(''),
    [speaker, setSpeaker] = useState('')
  const [myName, setMyName] = useState('')
  const [template, setTemplate] = useState('general')
  const [resumeId, setResumeId] = useState<string | null>(null)
  const [libraryTab, setLibraryTab] = useState('records')
  const [detailTab, setDetailTab] = useState('summary')
  const [searchSegment, setSearchSegment] = useState<string | null>(null)
  useEffect(() => {
    void meetingCall<Preferences>('meeting_preferences')
      .then((p) => {
        setMyName(p.myName)
        setTemplate(p.defaultTemplate || 'general')
      })
      .catch(() => {})
  }, [])
  const [summaryProvider, setSummaryProvider] = useState<string | null>(null)
  const [list, setList] = useState<MeetingList | null>(null)
  const [selectedId, setSelectedId] = useState<string | null>(null)
  useEffect(() => {
    const id = new URLSearchParams(location.search).get('meeting')
    if (id && /^[0-9a-f-]{36}$/i.test(id)) {
      setSelectedId(id)
      setDetailTab('summary')
      setError(null)
    }
  }, [location.key, location.search])
  const [detail, setDetail] = useState<Meeting | null>(null)
  const [query, setQuery] = useState('')
  const [error, setShownError] = useState<string | null>(null)
  // The last error shown by background polling: shown once, and cleared when polling recovers.
  const pollingError = useRef<string | null>(null)
  // Once the shown error is dismissed or cleared, a polling failure that persists shows again.
  const setError = useCallback((message: string | null) => {
    if (message === null) pollingError.current = null
    setShownError(message)
  }, [])
  const [busy, setBusy] = useState(false)
  const [creating, setCreating] = useState(false)
  useEffect(() => {
    if (new URLSearchParams(location.search).get('start') === '1') setCreating(true)
  }, [location.key, location.search])
  const [importChoice, setImportChoice] = useState<ImportChoice | null>(null)
  const [title, setTitle] = useState('')
  const [language, setLanguage] = useState('auto')
  const [source, setSource] = useState('microphone')
  const [appPid, setAppPid] = useState<number | null>(null)
  const [modelId, setModelId] = useState('')
  const [start, setStart] = useState('0')
  const [length, setLength] = useState('60')
  const [removing, setRemoving] = useState(false)
  const [editing, setEditing] = useState<Meeting | null>(null)
  const [evidence, setEvidence] = useState<{ meetingId: string; segment: MeetingSegment } | null>(
    null,
  )
  const [reviewing, setReviewing] = useState<{ meeting: Meeting; index: number } | null>(null)
  const [editingIndex, setEditingIndex] = useState<number | null>(null)
  useEffect(() => {
    if (editing && editingIndex !== null) {
      const frame = requestAnimationFrame(() => {
        document.getElementById(`edit-meeting-item-${editingIndex}`)?.focus()
      })
      return () => cancelAnimationFrame(frame)
    }
  }, [editing?.id, editingIndex])
  const refreshGeneration = useRef(0)
  const refreshing = useRef<Promise<void> | null>(null)
  const refreshQueued = useRef(false)
  const selectedRef = useRef(selectedId)
  selectedRef.current = selectedId
  const searchRef = useRef(location.search)
  searchRef.current = location.search
  const setSearchParamsRef = useRef(setSearchParams)
  setSearchParamsRef.current = setSearchParams
  const load = useCallback(async () => {
    const generation = refreshGeneration.current
    const id = selectedRef.current
    const [listed, fetched] = await Promise.allSettled([
      meetingCall<MeetingList>('meeting_list'),
      id ? meetingCall<Meeting>('meeting_get', { id }) : Promise.resolve(null),
    ])
    if (generation !== refreshGeneration.current) return
    const next = listed.status === 'fulfilled' ? listed.value : null
    if (next)
      setList((prev) => (prev && meetingListKey(prev) === meetingListKey(next) ? prev : next))
    let failure: unknown = listed.status === 'rejected' ? listed.reason : null
    if (selectedRef.current === id) {
      if (fetched.status === 'fulfilled') {
        const meeting = fetched.value
        setDetail((prev) =>
          prev && meeting && meetingKey(prev) === meetingKey(meeting) ? prev : meeting,
        )
      } else if (id && next && !next.meetings.some((m) => m.id === id)) {
        // Deleted meeting or an outdated link: return to the list and say so once.
        setSelectedId(null)
        setDetail(null)
        if (new URLSearchParams(searchRef.current).get('meeting'))
          setSearchParamsRef.current({}, { replace: true })
        pollingError.current = null
        setError(meetingMissingMessage)
        return
      } else failure ??= fetched.reason
    }
    if (failure) {
      const message = meetingError(failure)
      if (pollingError.current !== message) {
        pollingError.current = message
        setError(message)
      }
    } else if (pollingError.current) {
      const previous = pollingError.current
      pollingError.current = null
      setShownError((current) => (current === previous ? null : current))
    }
  }, [])
  // One request at a time; calls made meanwhile coalesce into a single trailing refresh, and
  // the returned promise settles after it, so callers still see their own changes.
  const refresh = useCallback((): Promise<void> => {
    if (refreshing.current) {
      refreshQueued.current = true
      return refreshing.current
    }
    const task = (async () => {
      do {
        refreshQueued.current = false
        await load()
      } while (refreshQueued.current)
    })().finally(() => {
      refreshing.current = null
    })
    refreshing.current = task
    return task
  }, [load])
  useEffect(() => {
    let closed = false
    let off: (() => void) | undefined
    if (isTauri())
      void listen('meetings_changed', () => {
        if (!closed) void refresh()
      }).then((fn) => {
        if (closed) fn()
        else off = fn
      })
    void refresh()
    const interval = setInterval(() => void refresh(), 1500)
    return () => {
      closed = true
      off?.()
      clearInterval(interval)
      ++refreshGeneration.current
    }
  }, [refresh])
  useEffect(() => {
    // Dialogs belong to the meeting they were opened from.
    setDetail(null)
    setEvidence(null)
    setEditing(null)
    setReviewing(null)
    setRemoving(false)
    void refresh()
  }, [selectedId, refresh])
  const run = async (task: () => Promise<void>) => {
    if (busy) return
    setBusy(true)
    setError(null)
    try {
      await task()
      await refresh()
    } catch (e) {
      setError(meetingError(e))
    } finally {
      setBusy(false)
    }
  }
  const active = list?.active
  const locked =
    busy ||
    (!!active && active.meetingId === selectedId) ||
    (!!detail && ['transcribing', 'summarizing'].includes(detail.state))
  const libraryVersion = list?.meetings.map((m) => `${m.id}:${m.revision}`).join(',') || ''
  const filteredMeetings = useMemo(
    () =>
      list?.meetings.filter(
        (m) =>
          (!filterProject ||
            (m.project || '').toLowerCase().includes(filterProject.trim().toLowerCase())) &&
          (!speaker ||
            m.participants?.some((name) => name.toLowerCase().includes(speaker.toLowerCase())) ||
            m.segments.some((s) => s.speaker?.toLowerCase().includes(speaker.toLowerCase())) ||
            m.items.some((i) => i.owner?.toLowerCase().includes(speaker.toLowerCase()))) &&
          (!after || m.createdAt >= new Date(`${after}T00:00:00`).getTime()) &&
          (!before || m.createdAt <= new Date(`${before}T23:59:59.999`).getTime()),
      ) || [],
    [list, filterProject, speaker, after, before],
  )
  const segmentById = useMemo(() => new Map(detail?.segments.map((s) => [s.id, s]) ?? []), [detail])
  const transcriptRows = useMemo(
    () =>
      detail?.segments.map((s) => (
        <div
          id={`segment-${s.id}`}
          key={s.id}
          className={`flex gap-5 rounded-md p-2 ${searchSegment === s.id ? 'bg-accent' : ''}`}
        >
          <button
            disabled={detail.audioDeleted}
            onClick={() => setEvidence({ meetingId: detail.id, segment: s })}
            className="h-fit shrink-0 rounded py-1 text-sm text-muted-foreground tabular-nums hover:text-foreground"
          >
            {timecode(s.startMs)}
          </button>
          <div>
            {s.speaker && (
              <p className="mb-1 text-xs font-medium text-muted-foreground">{s.speaker}</p>
            )}
            <p className="text-sm leading-8 whitespace-pre-wrap">{s.text}</p>
          </div>
        </div>
      )),
    [detail, searchSegment],
  )
  const openRecord = (id: string, segmentId?: string | null) => {
    setSelectedId(id)
    setSearchSegment(segmentId || null)
    setDetailTab(segmentId ? 'transcript' : 'summary')
  }
  useEffect(() => {
    if (detail && searchSegment)
      document.getElementById(`segment-${searchSegment}`)?.scrollIntoView({ block: 'center' })
  }, [detail?.id, detailTab, searchSegment])
  const currentActive = active?.meetingId === selectedId ? active : null
  const recording = currentActive?.kind === 'recording'
  const chooseImport = () =>
    void run(async () => {
      const choice = await meetingCall<ImportChoice | null>('meeting_pick')
      if (choice) {
        setImportChoice(choice)
        setTitle(choice.name)
        setStart('0')
        setLength(String(Math.min(choice.durationSeconds, 3600) / 60))
      }
    })
  const saveEdit = () =>
    void run(async () => {
      if (!editing) return
      await meetingCall('meeting_update', {
        id: editing.id,
        revision: editing.revision,
        title: editing.title,
        items: editing.items,
      })
      setEditing(null)
      toast.success(t('已保存修改', 'Changes saved'))
    })
  const updateItem = (index: number, patch: Partial<MeetingItem>) =>
    setEditing((m) =>
      m ? { ...m, items: m.items.map((i, n) => (n === index ? { ...i, ...patch } : i)) } : m,
    )
  const openEvidence = (s: MeetingSegment) => {
    if (detail) setEvidence({ meetingId: detail.id, segment: s })
  }

  return (
    <>
      {selectedId ? (
        <>
          <Button
            variant="ghost"
            size="sm"
            className="mb-6 -ml-3"
            onClick={() => {
              setSearchParams({}, { replace: true })
              setSelectedId(null)
              setEditing(null)
            }}
          >
            <PiArrowLeft />
            {t('会议记录', 'Meetings')}
          </Button>
          {detail && (
            <>
              <PageTitle
                title={detail.title}
                description={`${new Date(detail.createdAt).toLocaleString(t('zh-CN', 'en'))} · ${timecode(detail.durationMs)} · ${meetingStatusLabel(detail, t)}`}
                action={
                  <div className="flex flex-wrap gap-2">
                    <Button
                      variant="outline"
                      disabled={busy || !detail.items.length}
                      onClick={() =>
                        void run(async () => {
                          await meetingCall('meeting_copy', { id: detail.id, transcript: false })
                          toast.success(t('已复制纪要', 'Notes copied'))
                        })
                      }
                    >
                      <PiCopy />
                      {t('复制纪要', 'Copy notes')}
                    </Button>
                    <Button
                      variant="outline"
                      disabled={locked}
                      onClick={() => {
                        setEditingIndex(null)
                        setEditing(structuredClone(detail))
                      }}
                    >
                      <PiPencilSimple />
                      {t('编辑', 'Edit')}
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      aria-label={t('删除会议', 'Delete meeting')}
                      disabled={locked}
                      onClick={() => setRemoving(true)}
                    >
                      <PiTrash />
                    </Button>
                  </div>
                }
              />
              <TextServiceChoice
                meeting={detail}
                disabled={locked}
                onChange={(id) => {
                  setSummaryProvider(id)
                  void refresh()
                }}
              />
              <div className="mb-5 flex flex-wrap items-center gap-3">
                <TemplateChoice
                  value={detail.template}
                  disabled={locked}
                  onChange={(template) =>
                    void run(async () => {
                      await meetingCall('meeting_template_choose', {
                        id: detail.id,
                        revision: detail.revision,
                        template,
                      })
                    })
                  }
                />
                <WorkspacePreferences onSaved={(p) => setMyName(p.myName)} />
              </div>
              <MeetingWorkbench
                key={detail.id}
                meeting={detail}
                meetings={list?.meetings || []}
                recording={!!recording}
                disabled={locked}
                refresh={refresh}
                open={openRecord}
              />
              <PersonalResult
                meeting={detail}
                name={myName}
                onEvidence={(id) => {
                  const segment = segmentById.get(id)
                  if (segment) openEvidence(segment)
                }}
              />
              {detail.gaps?.length > 0 && (
                <div role="status" className="mb-5 rounded-lg border p-4 text-sm">
                  <p className="mb-2 font-medium">{t('录音中断记录', 'Recording interruptions')}</p>
                  {detail.gaps.map((gap, n) => (
                    <p className="mb-1 leading-6 text-muted-foreground" key={n}>
                      {timecode(gap.afterMs)} · {gap.reason} ·{' '}
                      {new Date(gap.startedAt).toLocaleTimeString()} —{' '}
                      {gap.endedAt
                        ? new Date(gap.endedAt).toLocaleTimeString()
                        : t('尚未续录', 'Not resumed yet')}
                      {gap.endedAt &&
                        ` (${Math.max(0, Math.round((gap.endedAt - gap.startedAt) / 1000))}${t('秒未录音', ' seconds unrecorded')})`}
                    </p>
                  ))}
                </div>
              )}
              {!recording && !detail.audioDeleted && detail.source !== 'import' && (
                <div className="mb-5 flex flex-wrap items-center gap-2">
                  <Button
                    variant="outline"
                    disabled={
                      busy ||
                      (!!active && !['processing', 'captions', 'download'].includes(active.kind))
                    }
                    onClick={() =>
                      void run(async () => {
                        await meetingCall('meeting_resume', {
                          id: detail.id,
                          source: detail.source,
                          appPid: null,
                          resetScope: false,
                        })
                      })
                    }
                  >
                    {t('续录这场会议', 'Resume this meeting')}
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={
                      busy ||
                      (!!active && !['processing', 'captions', 'download'].includes(active.kind))
                    }
                    onClick={() => {
                      setResumeId(detail.id)
                      setTitle(detail.title)
                      setSource(detail.source)
                      setAppPid(null)
                      setCreating(true)
                    }}
                  >
                    {t('调整音源后续录', 'Adjust audio source and resume')}
                  </Button>
                  {detail.systemApp && (
                    <span className="text-xs text-muted-foreground">{detail.systemApp.name}</span>
                  )}
                </div>
              )}
              {recording && (
                <div className="mb-8 rounded-2xl bg-muted/60 px-6 py-6">
                  <div className="flex items-center justify-between gap-4">
                    <div>
                      <div className="flex items-center gap-2 text-sm">
                        <PiMicrophone />
                        {active?.paused
                          ? t('录音已暂停', 'Recording paused')
                          : t('正在记录会议', 'Recording meeting')}
                      </div>
                      <p className="mt-3 text-4xl font-medium tracking-tight tabular-nums">
                        <LiveRecordingTime
                          meetingId={detail.id}
                          fallbackMs={currentActive?.elapsedMs ?? detail.durationMs}
                        />
                      </p>
                    </div>
                    <div className="flex gap-2">
                      <Button
                        variant="outline"
                        disabled={busy}
                        onClick={() =>
                          void run(async () => {
                            await meetingCall('meeting_control', {
                              id: detail.id,
                              action: active?.paused ? 'resume' : 'pause',
                            })
                          })
                        }
                      >
                        {active?.paused ? <PiPlay /> : <PiPause />}
                        {active?.paused ? t('继续', 'Resume') : t('暂停', 'Pause')}
                      </Button>
                      <Button
                        disabled={busy}
                        onClick={() =>
                          void run(async () => {
                            await meetingCall('meeting_control', { id: detail.id, action: 'stop' })
                          })
                        }
                      >
                        <PiStop />
                        {t('结束并整理', 'Finish and summarize')}
                      </Button>
                    </div>
                  </div>
                  <p className="mt-5 text-sm text-muted-foreground">
                    {t(
                      '录音持续保存在本机，不会因会议时长自动结束。离开此页面仍会继续录音。',
                      'Audio is saved locally as you record. Recording continues until you finish, even when you leave this page.',
                    )}
                  </p>
                </div>
              )}
              {currentActive && !recording && (
                <div className="mb-6 flex items-center justify-between gap-4 rounded-lg bg-muted px-4 py-3 text-sm">
                  <span>
                    {meetingStatus(t)[detail.state] ?? t('正在处理', 'Processing')}
                    {detail.state === 'transcribing' && detail.durationMs > 0
                      ? ` · ${Math.min(100, Math.round((detail.processedSamples / 16 / detail.durationMs) * 100))}%`
                      : ''}
                    {t('，可以离开此页面。', '. You can leave this page.')}
                  </span>
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() =>
                      void run(async () => {
                        await meetingCall('meeting_control', { id: detail.id, action: 'cancel' })
                      })
                    }
                  >
                    {t('停止处理', 'Stop processing')}
                  </Button>
                </div>
              )}
              {detail.error && (
                <p role="status" className="mb-5 rounded-lg bg-muted px-4 py-3 text-sm">
                  {meetingMessage(detail.error, t)}
                </p>
              )}
              {!currentActive &&
                detail.durationMs > 0 &&
                (!detail.audioDeleted || detail.segments.length > 0) && (
                  <div className="mb-7 flex flex-wrap items-center gap-3">
                    <Button
                      disabled={locked}
                      onClick={() =>
                        void run(async () => {
                          await meetingCall('meeting_process', { id: detail.id })
                        })
                      }
                    >
                      {detail.items.length
                        ? t('重新生成纪要', 'Regenerate notes')
                        : t('继续整理', 'Continue processing')}
                    </Button>
                    <Button
                      variant="ghost"
                      disabled={busy || !detail.segments.length}
                      onClick={() =>
                        void run(async () => {
                          await meetingCall('meeting_export', { id: detail.id, format: 'txt' })
                        })
                      }
                    >
                      {t('导出逐字稿', 'Export transcript')}
                    </Button>
                    <Button
                      variant="ghost"
                      disabled={busy || !detail.items.length}
                      onClick={() =>
                        void run(async () => {
                          await meetingCall('meeting_export', { id: detail.id, format: 'md' })
                        })
                      }
                    >
                      <PiDownloadSimple />
                      {t('导出 Markdown', 'Export Markdown')}
                    </Button>
                    <Button
                      variant="ghost"
                      disabled={busy || !detail.segments.length}
                      onClick={() =>
                        void run(async () => {
                          await meetingCall('meeting_export', { id: detail.id, format: 'json' })
                        })
                      }
                    >
                      {t('导出 JSON', 'Export JSON')}
                    </Button>
                  </div>
                )}
              <Select
                value=""
                disabled={locked}
                onValueChange={(value) =>
                  void run(async () => {
                    await meetingCall('meeting_export', { id: detail.id, format: value })
                  })
                }
              >
                <SelectTrigger
                  aria-label={t('更多导出格式', 'More export formats')}
                  className="mb-3 w-[170px]"
                >
                  <SelectValue placeholder={t('更多导出', 'More exports')} />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="csv">{t('待办与事项 CSV', 'Action items (CSV)')}</SelectItem>
                  <SelectItem value="srt">{t('字幕 SRT', 'Captions (SRT)')}</SelectItem>
                  <SelectItem value="vtt">{t('字幕 VTT', 'Captions (VTT)')}</SelectItem>
                </SelectContent>
              </Select>
              <MeetingExtras meeting={detail} disabled={locked} refresh={refresh} />
              <Tabs value={detailTab} onValueChange={setDetailTab}>
                <TabsList variant="line" className="mb-7">
                  <TabsTrigger value="summary">{t('纪要', 'Notes')}</TabsTrigger>
                  <TabsTrigger value="transcript">{t('逐字稿', 'Transcript')}</TabsTrigger>
                  <TabsTrigger value="assistant">{t('AI 助手', 'AI assistant')}</TabsTrigger>
                </TabsList>
                <TabsContent value="summary">
                  {detail.items.length > 0 ? (
                    <div className="max-w-[880px] space-y-9">
                      {detail.items.some((item) => item.support === 'review') && (
                        <p className="rounded-lg border p-4 text-sm leading-6 text-muted-foreground">
                          {t(
                            '纪要已生成。标为“需核对原文”的内容，AI 尚不能确定原文是否充分支持；可点开对照、修改，或稍后再看。',
                            'Your notes are ready. Items marked “Check against transcript” need a source check. Open an item to compare, edit, or review later.',
                          )}
                        </p>
                      )}
                      {groups.map(([kind, label]) => (
                        <section key={kind}>
                          <h2 className="mb-4 text-lg font-medium">{label}</h2>
                          {detail.items.filter((i) => i.kind === kind).length === 0 ? (
                            <p className="text-sm text-muted-foreground">
                              {t('未发现明确内容。', 'No confirmed items found.')}
                            </p>
                          ) : (
                            <div className="divide-y">
                              {detail.items.map(
                                (item, index) =>
                                  item.kind === kind && (
                                    <div
                                      key={index}
                                      className="flex items-start gap-3 py-4 first:pt-0"
                                    >
                                      {kind === 'todo' && (
                                        <Checkbox
                                          aria-label={t(
                                            `完成：${item.text}`,
                                            `Complete: ${item.text}`,
                                          )}
                                          className="mt-1 size-4"

                                          checked={item.done}
                                          disabled={locked}
                                          onCheckedChange={(value) => {
                                            const items = detail.items.map((i, n) =>
                                              n === index ? { ...i, done: value === true } : i,
                                            )
                                            void run(async () => {
                                              await meetingCall('meeting_update', {
                                                id: detail.id,
                                                revision: detail.revision,
                                                title: detail.title,
                                                items,
                                              })
                                            })
                                          }}
                                        />
                                      )}
                                      <div className="min-w-0 flex-1">
                                        <ItemState item={item} />
                                        {(item.support === 'review' || item.reviewed) && (
                                          <Button
                                            variant="ghost"
                                            size="sm"
                                            className="mb-2 px-0 text-xs"
                                            disabled={locked}
                                            onClick={() =>
                                              setReviewing({
                                                meeting: structuredClone(detail),
                                                index,
                                              })
                                            }
                                          >
                                            {item.reviewed
                                              ? t('查看核对结果', 'View review')
                                              : t('查看原文并核对', 'View transcript and check')}
                                          </Button>
                                        )}
                                        <p
                                          className={`text-sm leading-7 ${item.done ? 'text-muted-foreground line-through' : ''}`}
                                        >
                                          {item.text}
                                        </p>
                                        {kind === 'todo' && (
                                          <p className="mt-1 text-xs text-muted-foreground">
                                            {item.owner ??
                                              t('负责人待确认', 'Owner to be confirmed')}{' '}
                                            ·{' '}
                                            {item.due ??
                                              t('截止日期未指定', 'No due date specified')}
                                          </p>
                                        )}
                                      </div>
                                      <div className="flex shrink-0 flex-col gap-1">
                                        {item.evidence.map((id) => {
                                          const s = segmentById.get(id)
                                          return (
                                            s && (
                                              <button
                                                key={id}
                                                aria-label={t(
                                                  `查看 ${timecode(s.startMs)} 的原文`,
                                                  `View transcript at ${timecode(s.startMs)}`,
                                                )}
                                                title={t(
                                                  '查看原文，可回听这段录音',
                                                  'View transcript and listen to this excerpt',
                                                )}
                                                className="rounded px-2 py-1 text-xs text-muted-foreground tabular-nums hover:bg-muted hover:text-foreground"
                                                onClick={() => openEvidence(s)}
                                              >
                                                {timecode(s.startMs)}
                                              </button>
                                            )
                                          )
                                        })}
                                      </div>
                                    </div>
                                  ),
                              )}
                            </div>
                          )}
                        </section>
                      ))}
                    </div>
                  ) : (
                    <EmptyState
                      icon={PiNotepad}
                      title={
                        currentActive
                          ? t('会议正在处理中', 'Processing your meeting')
                          : detail.state === 'ready'
                            ? t('未发现明确的会议事项', 'No confirmed meeting items found')
                            : t('纪要将在这里呈现', 'Your meeting notes will appear here')
                      }
                      description={
                        detail.state === 'ready'
                          ? t(
                              '没有足够依据时不会生成结论，可切换到逐字稿查看。',
                              'No conclusions are generated without sufficient evidence. View the transcript for details.',
                            )
                          : detail.segments.length
                            ? t(
                                '逐字稿已保留，使用所选模型提取决策、待办与关键结论。',
                                'Your transcript is saved. Use the selected model to extract decisions, action items and key findings.',
                              )
                            : t(
                                '结束录音后会先转写，再生成可编辑的会议纪要。',
                                'After recording, audio is transcribed and turned into editable meeting notes.',
                              )
                      }
                    />
                  )}
                </TabsContent>
                <TabsContent value="assistant">
                  <ConversationAssistant meetingId={detail.id} contextKey={detail.id} />
                </TabsContent>
                <TabsContent value="transcript">
                  <SpeakerNames meeting={detail} disabled={locked} refresh={refresh} />
                  <div className="max-w-[900px] space-y-5">
                    {detail.segments.length ? (
                      transcriptRows
                    ) : (
                      <p className="text-sm text-muted-foreground">
                        {t(
                          '转写完成的片段会显示在这里。',
                          'Transcribed segments will appear here.',
                        )}
                      </p>
                    )}
                  </div>
                </TabsContent>
              </Tabs>
            </>
          )}
        </>
      ) : (
        <>
          <PageTitle
            title={t('会议记录', 'Meetings')}
            description={t(
              '留下决定，让下一步更清楚。',
              'Capture decisions. Make the next step clear.',
            )}
            action={
              <div className="flex gap-2">
                <Button
                  variant="outline"
                  disabled={
                    busy ||
                    (!!active && !['captions', 'download', 'processing'].includes(active.kind))
                  }
                  onClick={chooseImport}
                >
                  <PiUploadSimple />
                  {t('导入录音', 'Import audio')}
                </Button>
                <Button
                  disabled={
                    busy ||
                    (!!active && !['captions', 'download', 'processing'].includes(active.kind))
                  }
                  onClick={() => {
                    setTitle('')
                    setResumeId(null)
                    setCreating(true)
                  }}
                >
                  <PiPlus />
                  {t('开始会议', 'Start meeting')}
                </Button>
              </div>
            }
          />
          <TextServiceChoice disabled={locked} onChange={setSummaryProvider} />
          {active?.meetingId && (
            <button
              className="mb-6 flex w-full items-center justify-between rounded-xl bg-muted p-4 text-left text-sm"
              onClick={() => setSelectedId(active.meetingId)}
            >
              <span>
                {active.kind === 'recording'
                  ? t('会议录音进行中', 'Meeting recording in progress')
                  : t('会议正在处理', 'Meeting processing in progress')}
              </span>
              <span>{t('查看进度', 'View progress')}</span>
            </button>
          )}
          <div className="mb-5 flex items-center justify-between gap-3">
            <Tabs value={libraryTab} onValueChange={setLibraryTab}>
              <TabsList>
                <TabsTrigger value="records">{t('所有会议', 'All meetings')}</TabsTrigger>
                <TabsTrigger value="tasks">{t('我的跟进', 'Follow-ups')}</TabsTrigger>
              </TabsList>
            </Tabs>
            {libraryTab === 'records' && (
              <WorkspacePreferences
                onSaved={(p) => {
                  setMyName(p.myName)
                  setTemplate(p.defaultTemplate || 'general')
                }}
              />
            )}
          </div>
          {libraryTab === 'tasks' && (
            <AcrossMeetingTasks
              version={libraryVersion}
              name={myName}
              open={openRecord}
              identityControl={
                <WorkspacePreferences
                  triggerLabel={
                    myName.trim()
                      ? t('修改我的身份', 'Change my identity')
                      : t('设置我的身份', 'Set my identity')
                  }
                  onSaved={(p) => {
                    setMyName(p.myName)
                    setTemplate(p.defaultTemplate || 'general')
                  }}
                />
              }
            />
          )}
          {libraryTab === 'records' && (
            <>
              <Input
                aria-label={t('搜索会议', 'Search meetings')}
                className="mb-5 max-w-sm"
                placeholder={t(
                  '搜索原文、纪要、标题、负责人…',
                  'Search transcript, notes, titles, owners…',
                )}
                value={query}
                maxLength={200}
                onChange={(e) => setQuery(e.target.value)}
              />
              <div className="mb-4 flex flex-wrap gap-3">
                <Input
                  className="w-40"
                  value={filterProject}
                  onChange={(e) => setFilterProject(e.target.value)}
                  aria-label={t('筛选项目', 'Filter project')}
                  placeholder={t('项目 / 会议系列', 'Project / meeting series')}
                />
                <Input
                  className="w-40"
                  value={speaker}
                  onChange={(e) => setSpeaker(e.target.value)}
                  aria-label={t('筛选人物', 'Filter person')}
                  placeholder={t('人物 / 负责人', 'Person / owner')}
                />
                <Input
                  className="w-40"
                  type="date"
                  aria-label={t('开始日期', 'From date')}
                  value={after}
                  onChange={(e) => setAfter(e.target.value)}
                />
                <Input
                  className="w-40"
                  type="date"
                  aria-label={t('结束日期', 'Through date')}
                  value={before}
                  onChange={(e) => setBefore(e.target.value)}
                />
                {(filterProject || speaker || after || before) && (
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => {
                      setFilterProject('')
                      setSpeaker('')
                      setAfter('')
                      setBefore('')
                    }}
                  >
                    {t('清除筛选', 'Clear filters')}
                  </Button>
                )}
              </div>
              <LibraryResearch
                project={filterProject}
                after={after}
                before={before}
                speaker={speaker}
                open={openRecord}
              />
              {query.trim() ? (
                <FullTextResults
                  query={query}
                  version={libraryVersion}
                  open={openRecord}
                  filters={{ project: filterProject, after, before, speaker }}
                />
              ) : list && !list.meetings.length ? (
                <EmptyState
                  icon={PiNotepad}
                  title={t('从一场会议开始', 'Start with a meeting')}
                  description={t(
                    '录一段，或者导入已有录音。会议结束后，按所选模型整理决策、待办和关键结论。',
                    'Record a meeting or import audio. Use your selected model to organize decisions, action items and key findings.',
                  )}
                />
              ) : list && !filteredMeetings.length ? (
                <EmptyState
                  icon={PiNotepad}
                  title={t('没有符合筛选条件的会议', 'No meetings match these filters')}
                  description={t(
                    '换一个项目、人物或日期范围，或点击“清除筛选”查看全部会议。',
                    'Try another project, person or date range, or clear the filters to see all meetings.',
                  )}
                />
              ) : (
                <div className="divide-y">
                  {filteredMeetings.map((m) => (
                    <button
                      key={m.id}
                      className="flex w-full items-center justify-between gap-5 rounded-sm px-2 py-5 text-left transition-colors hover:bg-muted/50"
                      onClick={() => openRecord(m.id)}
                    >
                      <div>
                        <p className="text-sm font-medium">{m.title}</p>
                        <p className="mt-2 text-xs text-muted-foreground">
                          {new Date(m.createdAt).toLocaleDateString(t('zh-CN', 'en'))} ·{' '}
                          {timecode(m.durationMs)} ·{' '}
                          {{
                            import: t('导入录音', 'Import audio'),
                            microphone: t('麦克风', 'Microphone'),
                            system: t('系统声音', 'System audio'),
                            mixed: t('系统声音与麦克风', 'System audio and microphone'),
                          }[m.source] ?? m.source}
                        </p>
                      </div>
                      <span className="shrink-0 text-xs text-muted-foreground">
                        {meetingStatusLabel(m, t)}
                      </span>
                    </button>
                  ))}
                </div>
              )}
            </>
          )}
        </>
      )}
      {list && !list.textModelReady && !summaryProvider && (
        <div className="mt-10 flex flex-wrap items-center justify-between gap-4 rounded-xl bg-muted/50 p-5">
          <div>
            <p className="text-sm font-medium">
              {t('本地会议整理模型', 'Local meeting summary model')}
            </p>
            <p className="mt-1 text-xs leading-6 text-muted-foreground">
              {t(
                'Qwen3 4B · 约 2.5 GB。下载后可离线整理，录音与文字留在本机。',
                'Qwen3 4B · About 2.5 GB. Summarize offline after downloading; audio and text stay on your device.',
              )}
            </p>
            {list.downloadError && (
              <p className="mt-1 text-xs text-destructive">{list.downloadError}</p>
            )}
          </div>
          {list.downloadModel === 'qwen3' ? (
            <div className="flex items-center gap-3 text-sm">
              <span className="tabular-nums">
                {Math.floor((list.downloadedBytes / list.textModelBytes) * 100)}%
              </span>
              <Button
                variant="outline"
                size="sm"
                onClick={() =>
                  void run(async () => {
                    await meetingCall('meeting_text_cancel')
                  })
                }
              >
                {t('取消下载', 'Cancel download')}
              </Button>
            </div>
          ) : (
            <Button
              variant="outline"
              size="sm"
              disabled={locked}
              onClick={() =>
                void run(async () => {
                  await meetingCall('meeting_text_download')
                })
              }
            >
              <PiDownloadSimple />
              {t('下载模型', 'Download model')}
            </Button>
          )}
        </div>
      )}
      {error && !creating && (
        <div
          role="alert"
          className="mt-5 flex items-start justify-between gap-3 rounded-lg border border-destructive/20 bg-destructive/5 p-4 text-sm text-destructive"
        >
          <span>{meetingMessage(error, t)}</span>
          <button aria-label={t('关闭错误提示', 'Dismiss error')} onClick={() => setError(null)}>
            <PiX />
          </button>
        </div>
      )}

      <Dialog
        open={creating}
        onOpenChange={(open) => {
          if (!busy) setCreating(open)
        }}
      >
        <DialogContent
          closeLabel={t('关闭', 'Close')}
          showCloseButton={!busy}
          className="flex flex-col overflow-hidden"
        >
          <DialogHeader className="shrink-0">
            <DialogTitle>
              {resumeId ? t('续录这场会议', 'Resume this meeting') : t('开始会议', 'Start meeting')}
            </DialogTitle>
            <DialogDescription>
              {t(
                '选择录音来源，录音将持续到你结束会议。请先告知参会者正在录音。',
                'Choose an audio source. Recording continues until you finish. Let participants know you are recording.',
              )}
            </DialogDescription>
          </DialogHeader>
          <div className="-mx-1 min-h-0 flex-1 space-y-5 overflow-y-auto px-1 py-3">
            <label className="grid gap-3 text-sm">
              <span>{t('会议标题', 'Meeting title')}</span>
              <Input
                disabled={!!resumeId}
                maxLength={120}
                value={title}
                onChange={(e) => setTitle(e.target.value)}
                placeholder={t('例如：产品评审会议', 'For example: Product review')}
              />
            </label>
            {!resumeId && (
              <>
                <label className="grid gap-3 text-sm">
                  <span>{t('项目 / 会议系列（可选）', 'Project / meeting series (optional)')}</span>
                  <Input
                    maxLength={120}
                    value={project}
                    onChange={(e) => setProject(e.target.value)}
                    placeholder={t(
                      '同一项目的会议使用相同名称',
                      'Use the same name for related meetings',
                    )}
                  />
                </label>
                <PreparationCard
                  project={project}
                  meetings={list?.meetings || []}
                  open={(id) => {
                    setCreating(false)
                    openRecord(id)
                  }}
                />
                <TemplateChoice value={template} onChange={setTemplate} stacked />
              </>
            )}
            <AudioSource
              source={source}
              onSource={(next) => {
                setSource(next)
                setError(null)
              }}
              pid={appPid}
              onPid={setAppPid}
            />
            <MeetingMicrophoneAccess
              source={source}
              denied={error === '请在设置中授权麦克风。'}
              disabled={busy}
              onRecheck={() => setError(null)}
            />
            {!resumeId && (
              <>
                <label className="grid gap-3 text-sm">
                  <span>{t('会议转写模型', 'Meeting transcription model')}</span>
                  <Select
                    value={modelId || 'current'}
                    onValueChange={(value) => setModelId(value === 'current' ? '' : value)}
                  >
                    <SelectTrigger
                      aria-label={t('会议转写模型', 'Meeting transcription model')}
                      className="w-full"
                    >
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="current">
                        {t('使用当前语音模型', 'Use current speech model')}
                      </SelectItem>
                      <SelectItem value="moss">
                        {t('MOSS · 区分说话人', 'MOSS · Speaker identification')}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </label>
                {modelId === 'moss' && <MossSetup />}
                <label className="grid gap-3 text-sm">
                  <span>{t('会议语言', 'Meeting language')}</span>
                  <Select value={language} onValueChange={setLanguage}>
                    <SelectTrigger
                      aria-label={t('会议语言', 'Meeting language')}
                      className="w-full"
                    >
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="auto">{t('自动识别', 'Auto-detect')}</SelectItem>
                      <SelectItem value="zh">{t('中文', 'Chinese')}</SelectItem>
                      <SelectItem value="en">{t('英文', 'English')}</SelectItem>
                    </SelectContent>
                  </Select>
                </label>
              </>
            )}
            <p className="text-xs leading-6 text-muted-foreground">
              {t(
                '沿用设置中的麦克风及语音模型。',
                'Uses the microphone and speech model selected in settings.',
              )}
              <Link
                to="/settings/audio"
                onClick={() => setCreating(false)}
                className="underline underline-offset-4"
              >
                {t('检查设备', 'Check devices')}
              </Link>
            </p>
          </div>
          {error && (
            <p role="alert" className="shrink-0 text-sm text-destructive">
              {meetingMessage(error, t)}
            </p>
          )}
          <DialogFooter className="shrink-0">
            <Button variant="outline" disabled={busy} onClick={() => setCreating(false)}>
              {t('取消', 'Cancel')}
            </Button>
            <Button
              disabled={
                busy || (!!active && !['captions', 'download', 'processing'].includes(active.kind))
              }
              onClick={() =>
                void run(async () => {
                  const id =
                    resumeId ||
                    (await meetingCall<string>('meeting_start', {
                      title,
                      language,
                      source,
                      appPid,
                      modelId: modelId || null,
                      template,
                      project,
                    }))
                  if (resumeId)
                    await meetingCall('meeting_resume', { id: resumeId, source, appPid })
                  setCreating(false)
                  setSelectedId(id)
                  setDetailTab('summary')
                  setSearchSegment(null)
                  setResumeId(null)
                })
              }
            >
              <PiMicrophone />
              {busy ? t('正在准备录音…', 'Preparing recording…') : t('开始录音', 'Start recording')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog
        open={!!importChoice}
        onOpenChange={(open) => {
          if (!open) setImportChoice(null)
        }}
      >
        <DialogContent closeLabel={t('关闭', 'Close')}>
          <DialogHeader>
            <DialogTitle>{t('导入会议录音', 'Import meeting audio')}</DialogTitle>
            <DialogDescription>
              {t(
                '每次可导入最多 60 分钟的录音区间。原文件会保留在原处。',
                'Import a section up to 60 minutes long. The original file stays in its current location.',
              )}
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-5 py-3">
            <TemplateChoice value={template} onChange={setTemplate} stacked />
            <label className="grid gap-3 text-sm">
              <span>{t('会议标题', 'Meeting title')}</span>
              <Input maxLength={120} value={title} onChange={(e) => setTitle(e.target.value)} />
            </label>
            <label className="grid gap-3 text-sm">
              <span>{t('会议模型', 'Meeting model')}</span>
              <Select
                value={modelId || 'current'}
                onValueChange={(value) => setModelId(value === 'current' ? '' : value)}
              >
                <SelectTrigger
                  aria-label={t('会议转写模型', 'Meeting transcription model')}
                  className="w-full"
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="current">
                    {t('使用当前语音模型', 'Use current speech model')}
                  </SelectItem>
                  <SelectItem value="moss">
                    {t('MOSS · 区分说话人', 'MOSS · Speaker identification')}
                  </SelectItem>
                </SelectContent>
              </Select>
            </label>
            <p className="text-sm text-muted-foreground">
              {t('文件时长', 'File duration')}
              {timecode((importChoice?.durationSeconds ?? 0) * 1000)}
            </p>
            {modelId === 'moss' && <MossSetup />}
            <div className="grid grid-cols-2 gap-4">
              <label className="grid gap-3 text-sm">
                <span>{t('开始位置（分钟）', 'Start position (minutes)')}</span>
                <Input
                  type="number"
                  min="0"
                  step="0.1"
                  value={start}
                  onChange={(e) => setStart(e.target.value)}
                />
              </label>
              <label className="grid gap-3 text-sm">
                <span>{t('导入长度（分钟）', 'Duration to import (minutes)')}</span>
                <Input
                  type="number"
                  min="0.01"
                  max="60"
                  step="0.1"
                  value={length}
                  onChange={(e) => setLength(e.target.value)}
                />
              </label>
            </div>
          </div>
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {meetingMessage(error, t)}
            </p>
          )}
          <DialogFooter>
            <Button variant="outline" onClick={() => setImportChoice(null)}>
              {t('取消', 'Cancel')}
            </Button>
            <Button
              disabled={
                busy || (!!active && !['captions', 'download', 'processing'].includes(active.kind))
              }
              onClick={() =>
                void run(async () => {
                  const id = await meetingCall<string>('meeting_import', {
                    modelId: modelId || null,
                    template,
                    choiceId: importChoice?.choiceId,
                    title,
                    startSeconds: Number(start) * 60,
                    lengthSeconds: Number(length) * 60,
                    language,
                  })
                  setImportChoice(null)
                  setSelectedId(id)
                })
              }
            >
              {t('导入并整理', 'Import and summarize')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog open={removing} onOpenChange={setRemoving}>
        <DialogContent closeLabel={t('关闭', 'Close')}>
          <DialogHeader>
            <DialogTitle>{t('删除这场会议？', 'Delete this meeting?')}</DialogTitle>
            <DialogDescription>
              {t(
                '会删除本机保存的录音、逐字稿和纪要。原始导入文件不受影响。',
                'This deletes the locally saved audio, transcript and notes. The original imported file is unaffected.',
              )}
            </DialogDescription>
          </DialogHeader>
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {meetingMessage(error, t)}
            </p>
          )}
          <DialogFooter>
            <Button variant="outline" onClick={() => setRemoving(false)}>
              {t('取消', 'Cancel')}
            </Button>
            <Button
              variant="destructive"
              disabled={locked}
              onClick={() =>
                void run(async () => {
                  await meetingCall('meeting_delete', { id: selectedId })
                  setRemoving(false)
                  setSelectedId(null)
                })
              }
            >
              {t('删除会议', 'Delete meeting')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog
        open={!!editing}
        onOpenChange={(open) => {
          if (!open) setEditing(null)
        }}
      >
        <DialogContent
          closeLabel={t('关闭', 'Close')}
          className="max-h-[85vh] overflow-y-auto sm:max-w-2xl"
        >
          <DialogHeader>
            <DialogTitle>{t('编辑会议纪要', 'Edit meeting notes')}</DialogTitle>
            <DialogDescription>
              {t(
                '修改保存在本机，原文依据保持关联。',
                'Changes are saved locally. Links to supporting transcript segments are kept.',
              )}
            </DialogDescription>
          </DialogHeader>
          {editing && (
            <div className="space-y-5">
              <label className="grid gap-3 text-sm">
                <span>{t('标题', 'Title')}</span>
                <Input
                  value={editing.title}
                  maxLength={120}
                  onChange={(e) => setEditing({ ...editing, title: e.target.value })}
                />
              </label>
              {editing.items.map((item, index) => (
                <div key={index} className="space-y-3 border-t pt-4">
                  <p className="text-xs text-muted-foreground">
                    {groups.find(([kind]) => kind === item.kind)?.[1]}
                  </p>
                  <textarea
                    id={`edit-meeting-item-${index}`}
                    aria-label={t(`条目 ${index + 1}`, `Item ${index + 1}`)}
                    className={`${fieldClass} min-h-24`}
                    value={item.text}
                    onChange={(e) => updateItem(index, { text: e.target.value })}
                  />
                  <p className="text-sm">{t('内容性质', 'Type of statement')}</p>
                  <Select
                    value={
                      item.status ||
                      (
                        {
                          decision: 'decision',
                          todo: 'commitment',
                          fact: 'information',
                          next: 'proposal',
                        } as const
                      )[item.kind]
                    }
                    onValueChange={(value) =>
                      updateItem(index, { status: value as MeetingItem['status'] })
                    }
                  >
                    <SelectTrigger
                      aria-label={t('内容性质', 'Type of statement')}
                      className="w-full"
                    >
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {[
                        ['proposal', t('提议', 'Proposal')],
                        ['decision', t('决定', 'Decision')],
                        ['commitment', t('承诺', 'Commitment')],
                        ['pending', t('待确认', 'Pending')],
                        ['information', t('信息', 'Information')],
                      ].map(([value, label]) => (
                        <SelectItem value={value} key={value}>
                          {label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  {item.kind === 'todo' && (
                    <div className="grid grid-cols-2 gap-3">
                      <label className="grid gap-3 text-sm">
                        <span>{t('负责人', 'Owner')}</span>
                        <Input
                          aria-label={t('负责人', 'Owner')}
                          placeholder={t('负责人待确认', 'Owner to be confirmed')}
                          value={item.owner ?? ''}
                          onChange={(e) => updateItem(index, { owner: e.target.value || null })}
                        />
                      </label>
                      <label className="grid gap-3 text-sm">
                        <span>{t('截止日期', 'Due date')}</span>
                        <Input
                          aria-label={t('截止日期', 'Due date')}
                          placeholder={t('截止日期未指定', 'No due date specified')}
                          value={item.due ?? ''}
                          onChange={(e) => updateItem(index, { due: e.target.value || null })}
                        />
                      </label>
                      <label className="col-span-2 grid gap-3 text-sm">
                        <span>{t('交付给谁（可选）', 'Recipient (optional)')}</span>
                        <Input
                          aria-label={t('承诺交付给谁', 'Commitment recipient')}
                          placeholder={t(
                            '交付给谁（明确提到才填）',
                            'Recipient (only if explicit)',
                          )}
                          value={item.recipient || ''}
                          onChange={(e) => updateItem(index, { recipient: e.target.value || null })}
                        />
                      </label>
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {meetingMessage(error, t)}
            </p>
          )}
          <DialogFooter>
            <Button variant="outline" onClick={() => setEditing(null)}>
              {t('取消', 'Cancel')}
            </Button>
            <Button disabled={locked} onClick={saveEdit}>
              <PiCheck />
              {t('保存修改', 'Save changes')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog
        open={!!evidence}
        onOpenChange={(open) => {
          if (!open) {
            setEvidence(null)
          }
        }}
      >
        <DialogContent closeLabel={t('关闭', 'Close')}>
          <DialogHeader>
            <DialogTitle>
              {t('原文依据', 'Supporting transcript')}
              {evidence && ` · ${timecode(evidence.segment.startMs)}`}
            </DialogTitle>
            <DialogDescription>
              {t(
                '时间戳定位到原文。说话人归属不确定时请回听确认。',
                'Timestamps locate source segments. Listen to verify uncertain speaker identities.',
              )}
            </DialogDescription>
          </DialogHeader>
          <p className="max-h-72 overflow-y-auto py-4 text-sm leading-8 whitespace-pre-wrap">
            {evidence?.segment.text}
          </p>
          {evidence && (
            <MeetingAudio
              key={`${evidence.meetingId}:${evidence.segment.id}:${evidence.segment.startMs}`}
              meetingId={evidence.meetingId}
              startMs={evidence.segment.startMs}
              deleted={detail?.id === evidence.meetingId && detail.audioDeleted}
            />
          )}
        </DialogContent>
      </Dialog>
      {reviewing && (
        <MeetingItemReview
          meeting={reviewing.meeting}
          index={reviewing.index}
          currentRevision={detail?.id === reviewing.meeting.id ? detail.revision : undefined}
          disabled={locked}
          refresh={refresh}
          onClose={() => setReviewing(null)}
          onEdit={() => {
            setEditing(structuredClone(reviewing.meeting))
            setEditingIndex(reviewing.index)
            setReviewing(null)
          }}
        />
      )}
    </>
  )
}
