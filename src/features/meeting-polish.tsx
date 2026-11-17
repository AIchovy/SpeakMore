import { CaptionShortReply } from './caption-tools'
import { useEffect, useRef, useState } from 'react'
import { useText } from '@/lib/app-state'
import { isTauri } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  meetingCall,
  meetingEditError,
  meetingError,
  timecode,
  type Meeting,
  type MeetingItem,
  type Insight,
  itemStatus,
} from './meetings-api'
import { type Preferences } from './meeting-workspace'
import { meetingMessage } from './meeting-messages'
import { toast } from 'sonner'

export function ItemState({ item }: { item: MeetingItem }) {
  const t = useText()
  const status = itemStatus(item)
  const label = {
    proposal: t('提议', 'Proposal'),
    decision: t('决定', 'Decision'),
    commitment: t('承诺', 'Commitment'),
    pending: t('待确认', 'Pending'),
    information: t('信息', 'Information'),
  }[status]
  return (
    <div className="mb-1 flex flex-wrap gap-2 text-[11px] text-muted-foreground">
      <span className="rounded bg-muted px-2 py-0.5">{label}</span>
      {item.edited && (
        <span>
          {t('已手动编辑 · 重新整理时保留', 'Manually edited · preserved on regeneration')}
        </span>
      )}
      {item.support === 'review' ? (
        <span>{t('需核对原文', 'Check against transcript')}</span>
      ) : item.reviewed ? (
        <span>{t('你已确认与原文一致', 'You confirmed this matches the transcript')}</span>
      ) : (
        item.support === 'supported' && <span>{t('有原文支持', 'Supported by transcript')}</span>
      )}
    </div>
  )
}
export function InsightResult({
  insight,
  open,
}: {
  insight: Insight
  open: (id: string, segmentId?: string | null) => void
}) {
  const t = useText(),
    [error, setError] = useState('')
  return (
    <div className="space-y-4">
      {insight.stale && (
        <p role="status" className="rounded-lg border p-3 text-xs leading-6">
          {t(
            '参考原文已经修改或删除，这张结果卡可能过时，请重新生成。',
            'Source excerpts changed or were deleted. This result may be outdated; regenerate it.',
          )}
        </p>
      )}
      <p className="whitespace-pre-wrap text-sm leading-7">{insight.text}</p>
      <Button
        size="sm"
        variant="outline"
        onClick={() =>
          void meetingCall('captions_copy', { text: insight.text })
            .then(() => {
              setError('')
              toast.success(t('结果已复制', 'Result copied'))
            })
            .catch((e) => setError(meetingError(e)))
        }
      >
        {t('复制结果', 'Copy result')}
      </Button>
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      )}
      {insight.sources.length > 0 && (
        <details>
          <summary className="cursor-pointer text-xs text-muted-foreground">
            {t('本次参考原文', 'Source excerpts')} · {insight.sources.length}
          </summary>
          <div className="mt-3 divide-y">
            {insight.sources.map((source, n) => (
              <button
                key={`${source.meetingId}-${source.segmentId}-${n}`}
                className="block w-full py-3 text-left"
                onClick={() => open(source.meetingId, source.segmentId)}
              >
                <p className="text-xs text-muted-foreground">
                  [{n + 1}] {source.title} · {new Date(source.createdAt).toLocaleDateString()} ·{' '}
                  {timecode(source.startMs)}
                </p>
                <p className="mt-1 text-sm leading-6">{source.text}</p>
              </button>
            ))}
          </div>
        </details>
      )}
    </div>
  )
}
export function PreparationCard({
  project,
  meetings,
  open,
}: {
  project: string
  meetings: Meeting[]
  open: (id: string, segmentId?: string | null) => void
}) {
  const t = useText()
  const [records, setRecords] = useState<Meeting[]>([])
  const version = meetings.map((m) => `${m.id}:${m.revision}`).join(',')
  useEffect(() => {
    const name = project.trim()
    if (!name) {
      setRecords([])
      return
    }
    // Wait for typing to settle; earlier records stay until the new ones arrive (records for
    // another project are filtered out below, so they are never shown under this name).
    let alive = true
    const timer = setTimeout(
      () =>
        void meetingCall<Meeting[]>('meeting_prepare', { project: name })
          .then((value) => {
            if (alive) setRecords(value)
          })
          .catch(() => {}),
      300,
    )
    return () => {
      alive = false
      clearTimeout(timer)
    }
  }, [project, version])
  const previous = records
    .filter((m) => project.trim() && m.project === project.trim())
    .sort((a, b) => b.createdAt - a.createdAt)[0]
  if (!previous) return null
  const decisions = previous.items.filter(
    (i) => i.kind === 'decision' && itemStatus(i) === 'decision' && i.support !== 'review',
  )
  const tasks = records
    .filter((m) => m.project === project.trim())
    .flatMap((m) =>
      m.items.filter((i) => i.kind === 'todo' && !i.done).map((item) => ({ meeting: m, item })),
    )
  const questions = records.flatMap((m) =>
    m.items
      .filter(
        (item) =>
          !item.done &&
          (['proposal', 'pending'].includes(itemStatus(item)) ||
            item.support === 'review' ||
            (item.kind === 'todo' && (!item.owner || !item.due))),
      )
      .map((item) => ({ meeting: m, item })),
  )
  return (
    <details className="rounded-lg border p-3 text-sm" open>
      <summary className="cursor-pointer font-medium">
        {t('会前两分钟', 'Two-minute preparation')} · {project}
      </summary>
      <p className="my-3 text-xs text-muted-foreground">
        {t('上次记录', 'Previous record')} · {previous.title} ·{' '}
        {new Date(previous.createdAt).toLocaleDateString()}
      </p>
      {decisions.slice(0, 3).map((item, n) => (
        <button
          key={n}
          className="mb-2 block text-left leading-6 hover:underline"
          onClick={() => open(previous.id, item.evidence[0])}
        >
          {t('上次决定：', 'Previous decision: ')}
          {item.text}
        </button>
      ))}
      {tasks.slice(0, 5).map(({ meeting, item }, n) => (
        <button
          key={n}
          className="mb-2 block text-left leading-6 hover:underline"
          onClick={() => open(meeting.id, item.evidence[0])}
        >
          {t('待跟进：', 'Follow up: ')}
          {item.text} · {item.owner || t('负责人待确认', 'Owner to confirm')}
        </button>
      ))}
      {questions.slice(0, 4).map(({ meeting, item }, n) => (
        <button
          key={`question-${n}`}
          className="mb-2 block text-left leading-6 hover:underline"
          onClick={() => open(meeting.id, item.evidence[0])}
        >
          {t('今天先确认：', 'Confirm today: ')}
          {item.text}
        </button>
      ))}
      {records.some((m) => m.summaryStale) && (
        <p className="mb-2 text-xs text-muted-foreground">
          {t(
            '部分历史结果待更新，请结合来源核对。',
            'Some earlier results need updating; check their sources.',
          )}
        </p>
      )}
      {!tasks.length && (
        <p className="text-xs text-muted-foreground">
          {t(
            '已记录的事项中暂无未完成待办；不代表所有问题都已解决。',
            'No open tasks in the recorded items; this does not establish that every issue is resolved.',
          )}
        </p>
      )}
    </details>
  )
}
export function MeetingWorkbench({
  meeting,
  meetings,
  recording,
  disabled,
  refresh,
  open,
}: {
  meeting: Meeting
  meetings: Meeting[]
  recording: boolean
  disabled: boolean
  refresh: () => Promise<unknown>
  open: (id: string, segmentId?: string | null) => void
}) {
  const t = useText(),
    [note, setNote] = useState(''),
    [pending, setPending] = useState(false),
    [generating, setGenerating] = useState(''),
    [error, setError] = useState(''),
    // The revision the project dialog was opened at, so a newer change is reported as a conflict.
    [config, setConfig] = useState<number | null>(null),
    [project, setProject] = useState(meeting.project || ''),
    [prompt, setPrompt] = useState(meeting.templatePrompt || ''),
    [result, setResult] = useState<Insight | null>(null),
    [resultTitle, setResultTitle] = useState(''),
    [previous, setPrevious] = useState(''),
    [image, setImage] = useState<string | null>(null)
  const [editingNote, setEditingNote] = useState<{
    id: string
    text: string
    previousText: string
  } | null>(null)
  const input = useRef<HTMLTextAreaElement>(null)
  const action = async (work: () => Promise<unknown>, describe = meetingError) => {
    if (pending) return
    setPending(true)
    setError('')
    try {
      await work()
      await refresh()
    } catch (e) {
      setError(describe(e))
    } finally {
      setPending(false)
    }
  }
  const add = (kind: string) =>
    action(async () => {
      await meetingCall('meeting_note_add', {
        id: meeting.id,
        text: kind === 'highlight' ? '' : note,
        kind,
      })
      if (kind !== 'highlight') setNote('')
      toast.success(
        kind === 'highlight'
          ? t('已标记当前讨论，整理时会重点参考', 'Marked this discussion for the summary')
          : t('便签已保存，整理纪要时会参考', 'Note saved for the next summary'),
      )
    })
  useEffect(() => {
    if (!isTauri()) return
    let off: (() => void) | undefined
    let alive = true
    void listen('meeting_priority_marked', () => void refresh()).then((v) => {
      if (alive) off = v
      else v()
    })
    return () => {
      alive = false
      off?.()
    }
  }, [refresh])
  const impacted = meeting.items.filter((i) =>
    i.evidence.some((id) => meeting.dirtySegments?.includes(id)),
  )
  const notes = meeting.notes || []
  const showInsight = (kind: string, title: string) =>
    action(async () => {
      setResultTitle(title)
      setGenerating(title)
      try {
        const value = await meetingCall<Insight>('meeting_insight', {
          id: meeting.id,
          kind,
          previousId: previous || null,
        })
        setResult(value)
      } finally {
        setGenerating('')
      }
    })
  return (
    <section className="mb-6 space-y-4">
      <div className="flex flex-wrap items-center gap-2">
        <Button
          variant="outline"
          size="sm"
          onClick={() => {
            setProject(meeting.project || '')
            setPrompt(meeting.templatePrompt || '')
            setConfig(meeting.revision)
          }}
          disabled={disabled}
        >
          {meeting.project
            ? t(`项目：${meeting.project}`, `Project: ${meeting.project}`)
            : t('设置项目与纪要格式', 'Set project and notes format')}
        </Button>
        <Button
          variant="outline"
          size="sm"
          disabled={pending || !meeting.segments.length}
          onClick={() => void showInsight('closing', t('会议收尾卡', 'Meeting wrap-up'))}
        >
          {t('检查未决事项', 'Check unresolved items')}
        </Button>
        <Button
          variant="outline"
          size="sm"
          disabled={pending || !meeting.segments.length}
          onClick={() => void showInsight('deliverable', t('可交付结果', 'Deliverable'))}
        >
          {t('按模板整理成果', 'Create a template-based summary')}
        </Button>
        <Select
          value={previous || 'project'}
          onValueChange={(v) => setPrevious(v === 'project' ? '' : v)}
        >
          <SelectTrigger
            aria-label={t('决策对比会议', 'Previous meeting to compare')}
            className="w-[200px]"
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="project">
              {t('本项目上次会议', 'Previous project meeting')}
            </SelectItem>
            {meetings
              .filter((m) => m.id !== meeting.id && m.createdAt < meeting.createdAt)
              .map((m) => (
                <SelectItem key={m.id} value={m.id}>
                  {m.title}
                </SelectItem>
              ))}
          </SelectContent>
        </Select>
        <Button
          variant="ghost"
          size="sm"
          disabled={pending || !meeting.segments.length || (!previous && !meeting.project)}
          onClick={() => void showInsight('changes', t('决策变化记录', 'Decision changes'))}
        >
          {t('比较决策变化', 'Compare decision changes')}
        </Button>
      </div>
      <p
        role={generating ? 'status' : undefined}
        className="text-xs leading-6 text-muted-foreground"
      >
        {generating
          ? t(`正在生成「${generating}」…`, `Generating ${generating}…`)
          : t(
              '检查未决事项会找出未明确的负责人、时间和结论；按模板整理会生成适合这类会议的结果。',
              'Check unresolved items to find missing owners, dates and conclusions. The template shapes the summary for this type of meeting.',
            )}
      </p>
      {!!meeting.dirtySegments?.length && (
        <div role="status" className="rounded-lg border p-4 text-sm">
          <p>
            {t(
              `已修改 ${meeting.dirtySegments.length} 处原文，${impacted.length} 条纪要或待办可能受影响。`,
              `${meeting.dirtySegments.length} transcript edits may affect ${impacted.length} notes or tasks.`,
            )}
          </p>
          <p className="mt-2 text-xs leading-6 text-muted-foreground">
            {t(
              '此前的 AI 回答与结果卡也可能过时。更新仅重整相关讨论，保留手动编辑条目。',
              'Earlier AI answers and result cards may be outdated. Update the affected discussion while preserving manual edits.',
            )}
          </p>
          {impacted.slice(0, 4).map((item, n) => (
            <p key={n} className="mt-1 text-xs">
              • {item.text}
            </p>
          ))}
          <Button
            className="mt-3"
            size="sm"
            variant="outline"
            disabled={disabled || pending}
            onClick={() =>
              void action(() =>
                meetingCall('meeting_regenerate_affected', {
                  id: meeting.id,
                  revision: meeting.revision,
                }),
              )
            }
          >
            {t('更新受影响的结果', 'Update affected results')}
          </Button>
        </div>
      )}
      <details className="rounded-lg border p-4" open={recording || undefined}>
        <summary className="cursor-pointer text-sm font-medium">
          {recording ? t('随手记一句', 'Note what matters') : t('我的会议便签', 'My meeting notes')}{' '}
          {notes.length > 0 && `· ${notes.length}`}
        </summary>
        <p className="my-3 text-xs leading-6 text-muted-foreground">
          {recording
            ? t(
                '写下你在意的事，例如“这个报价需要确认”。便签会关联前后 30 秒讨论，整理纪要时重点参考。',
                'Write what matters to you, such as “Confirm this quote.” The note links to the surrounding 30 seconds and guides the summary.',
              )
            : t(
                '补充你自己的想法或提醒，整理纪要时会参考，并标为“个人补充”。保存后需重新整理才会进入新纪要；它不会改动会议原文。',
                'Add your thoughts or reminders as a personal addition. Regenerate the notes to include it in the next summary. The transcript stays as recorded.',
              )}
        </p>
        <Textarea
          ref={input}
          aria-label={t('随手记', 'Personal note')}
          maxLength={1200}
          value={note}
          onChange={(e) => setNote(e.target.value)}
          placeholder={t('例如：这个报价需要确认', 'For example: Confirm this quote')}
          className="min-h-20"
          onKeyDown={(e) => {
            if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
              e.preventDefault()
              void add(recording ? 'note' : 'supplement')
            }
          }}
        />
        <div className="mt-3 flex flex-wrap gap-2">
          <Button
            size="sm"
            disabled={pending || !note.trim()}
            onClick={() => void add(recording ? 'note' : 'supplement')}
          >
            {recording ? t('记下这句', 'Save note') : t('保存便签', 'Save note')}
          </Button>
          {recording && (
            <Button
              size="sm"
              variant="outline"
              disabled={pending}
              onClick={() => void add('highlight')}
            >
              {t('标记当前重点', 'Mark this moment')}
            </Button>
          )}
          <Button
            size="sm"
            variant="ghost"
            disabled={pending}
            onClick={() =>
              void action(() => meetingCall('meeting_capture_image', { id: meeting.id }))
            }
          >
            {t('截取讨论画面', 'Capture a discussion image')}
          </Button>
          <span className="self-center text-xs text-muted-foreground">⌘/Ctrl + Enter</span>
        </div>
        {!recording && (
          <CaptionShortReply
            t={t}
            model="auto"
            language="zh-CN"
            meetingId={meeting.id}
            onSaved={() => void refresh()}
          />
        )}
        <div className="mt-4 max-h-64 overflow-y-auto divide-y">
          {notes.map((n) => (
            <div key={n.id} className="flex items-start justify-between gap-3 py-3">
              <div>
                <button
                  className="text-xs text-muted-foreground hover:underline"
                  onClick={() => {
                    const s = meeting.segments.find((s) => s.endMs >= n.atMs)
                    if (s) open(meeting.id, s.id)
                  }}
                >
                  {timecode(n.atMs)} ·{' '}
                  {n.kind === 'highlight'
                    ? t('重点标记', 'Highlight')
                    : n.kind === 'supplement'
                      ? t('个人补充', 'Personal addition')
                      : n.kind === 'screenshot'
                        ? t('讨论截图', 'Discussion image')
                        : t('会中随手记', 'Meeting note')}
                </button>
                <p className="mt-1 whitespace-pre-wrap text-sm leading-6">
                  {n.text || t('请优先覆盖这段讨论', 'Prioritize this discussion')}
                </p>
                {n.image && (
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() =>
                      void action(async () =>
                        setImage(
                          await meetingCall<string>('meeting_note_image', {
                            id: meeting.id,
                            noteId: n.id,
                          }),
                        ),
                      )
                    }
                  >
                    {t('查看截图', 'View image')}
                  </Button>
                )}
              </div>
              <Button
                size="sm"
                variant="ghost"
                disabled={pending}
                onClick={() => setEditingNote({ id: n.id, text: n.text, previousText: n.text })}
              >
                {t('编辑笔记', 'Edit note')}
              </Button>
              <Button
                size="sm"
                variant="ghost"
                disabled={pending}
                onClick={() =>
                  void action(() =>
                    meetingCall('meeting_note_remove', { id: meeting.id, noteId: n.id }),
                  )
                }
              >
                {t('移除', 'Remove')}
              </Button>
            </div>
          ))}
        </div>
      </details>
      {!!meeting.artifacts?.length && (
        <details>
          <summary className="cursor-pointer text-xs text-muted-foreground">
            {t('已生成的结果卡', 'Saved result cards')}
          </summary>
          {meeting.artifacts.map((v) => (
            <Button
              key={v.kind}
              size="sm"
              variant="ghost"
              onClick={() => {
                setResultTitle(
                  v.kind === 'changes'
                    ? t('决策变化记录', 'Decision changes')
                    : t('可交付结果', 'Deliverable'),
                )
                setResult(v)
              }}
            >
              {v.kind === 'changes'
                ? t('决策变化记录', 'Decision changes')
                : t('可交付结果', 'Deliverable')}{' '}
              · {new Date(v.at).toLocaleTimeString()}
            </Button>
          ))}
        </details>
      )}
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {meetingMessage(error, t)}
        </p>
      )}
      {pending && (
        <p role="status" className="text-xs text-muted-foreground">
          {t('正在处理，录音与字幕继续。', 'Working. Recording and captions continue.')}
        </p>
      )}
      <Dialog
        open={!!editingNote}
        onOpenChange={(v) => {
          if (!v) setEditingNote(null)
        }}
      >
        <DialogContent closeLabel={t('关闭', 'Close')}>
          <DialogHeader>
            <DialogTitle>{t('编辑个人笔记', 'Edit personal note')}</DialogTitle>
            <DialogDescription>
              {t(
                '个人补充与会议事实分别保留。',
                'Personal additions remain separate from meeting facts.',
              )}
            </DialogDescription>
          </DialogHeader>
          <Textarea
            maxLength={1200}
            aria-label={t('个人笔记内容', 'Personal note text')}
            value={editingNote?.text || ''}
            onChange={(e) => setEditingNote((n) => (n ? { ...n, text: e.target.value } : n))}
          />
          <Button
            disabled={pending || !editingNote?.text.trim()}
            onClick={() =>
              void action(async () => {
                if (!editingNote) return
                await meetingCall('meeting_note_update', {
                  id: meeting.id,
                  noteId: editingNote.id,
                  text: editingNote.text,
                  previousText: editingNote.previousText,
                })
                setEditingNote(null)
              })
            }
          >
            {t('保存笔记', 'Save note')}
          </Button>
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {meetingMessage(error, t)}
            </p>
          )}
        </DialogContent>
      </Dialog>
      <Dialog
        open={config !== null}
        onOpenChange={(open) => {
          if (!open) setConfig(null)
        }}
      >
        <DialogContent closeLabel={t('关闭', 'Close')}>
          <DialogHeader>
            <DialogTitle>{t('项目与模板要求', 'Project and template preferences')}</DialogTitle>
            <DialogDescription>
              {t(
                '关联同一项目，回顾上次会议并比较决策。模板要求只改变结果的组织方式。',
                'Link meetings to a project to review previous work and compare decisions. Template preferences affect presentation only.',
              )}
            </DialogDescription>
          </DialogHeader>
          <label className="grid gap-3 text-sm">
            <span>{t('项目 / 会议系列', 'Project / meeting series')}</span>
            <Input maxLength={120} value={project} onChange={(e) => setProject(e.target.value)} />
          </label>
          <label className="grid gap-3 text-sm">
            <span>{t('我想拿到的结果', 'My preferred result')}</span>
            <Textarea
              maxLength={2000}
              value={prompt}
              onChange={(e) => setPrompt(e.target.value)}
              placeholder={t(
                '例如：先列报价变化，再列双方承诺，最后给跟进草稿',
                'For example: Quote changes, then mutual commitments, then a follow-up draft',
              )}
            />
          </label>
          <div className="flex flex-wrap gap-2">
            <Button
              disabled={pending || disabled}
              onClick={() =>
                void action(async () => {
                  await meetingCall('meeting_context_save', {
                    id: meeting.id,
                    revision: config,
                    project,
                    prompt,
                  })
                  setConfig(null)
                }, meetingEditError)
              }
            >
              {t('用于本场', 'Use for this meeting')}
            </Button>
            <Button
              variant="outline"
              disabled={pending || disabled}
              onClick={() =>
                void action(async () => {
                  const p = await meetingCall<Preferences>('meeting_preferences')
                  await meetingCall('meeting_preferences_save', {
                    preferences: {
                      ...p,
                      templateRules: { ...p.templateRules, [meeting.template]: prompt },
                    },
                  })
                  await meetingCall('meeting_context_save', {
                    id: meeting.id,
                    revision: config,
                    project,
                    prompt,
                  })
                  setConfig(null)
                }, meetingEditError)
              }
            >
              {t('以后同类会议也这样整理', 'Use for future meetings of this type')}
            </Button>
          </div>
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {meetingMessage(error, t)}
            </p>
          )}
        </DialogContent>
      </Dialog>
      <Dialog
        open={!!result}
        onOpenChange={(v) => {
          if (!v) setResult(null)
        }}
      >
        <DialogContent closeLabel={t('关闭', 'Close')} className="sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>{resultTitle}</DialogTitle>
            <DialogDescription>
              {t(
                '基于已提供的原文片段，重要结论请点击来源核对。',
                'Based on the provided excerpts. Check important conclusions against their sources.',
              )}
            </DialogDescription>
          </DialogHeader>
          {result && (
            <InsightResult
              insight={result}
              open={(id, sid) => {
                setResult(null)
                open(id, sid)
              }}
            />
          )}
        </DialogContent>
      </Dialog>
      <Dialog
        open={!!image}
        onOpenChange={(v) => {
          if (!v) setImage(null)
        }}
      >
        <DialogContent closeLabel={t('关闭', 'Close')} className="sm:max-w-3xl">
          <DialogHeader>
            <DialogTitle>{t('讨论截图', 'Discussion image')}</DialogTitle>
            <DialogDescription>
              {t('由你主动截取，仅保存在本机。', 'Captured by you and stored locally.')}
            </DialogDescription>
          </DialogHeader>
          {image && (
            <img
              src={image}
              alt={t('关联本次讨论的截图', 'Image linked to this discussion')}
              className="w-full object-contain"
            />
          )}
        </DialogContent>
      </Dialog>
    </section>
  )
}
export function LibraryResearch({
  project,
  after,
  before,
  speaker,
  open,
}: {
  project: string
  after: string
  before: string
  speaker: string
  open: (id: string, segmentId?: string | null) => void
}) {
  const t = useText(),
    [question, setQuestion] = useState(''),
    [answeredQuestion, setAnsweredQuestion] = useState(''),
    [result, setResult] = useState<Insight | null>(null),
    [pending, setPending] = useState(false),
    [error, setError] = useState('')
  const request = useRef(0)
  useEffect(() => {
    request.current++
    setResult(null)
    setError('')
    setPending(false)
    return () => {
      request.current++
    }
  }, [project, after, before, speaker])
  const ask = async () => {
    if (pending || !question.trim()) return
    setPending(true)
    setError('')
    const token = ++request.current
    const asked = question
    try {
      const value = await meetingCall<Insight>('meeting_research', {
        query: question,
        project: project || null,
        after: after ? new Date(`${after}T00:00:00`).getTime() : null,
        before: before ? new Date(`${before}T23:59:59.999`).getTime() : null,
        speaker: speaker || null,
      })
      if (token === request.current) {
        setResult(value)
        setAnsweredQuestion(asked)
      }
    } catch (e) {
      if (token === request.current) setError(meetingError(e))
    } finally {
      if (token === request.current) setPending(false)
    }
  }
  return (
    <details className="my-5 rounded-lg border p-4">
      <summary className="cursor-pointer text-sm font-medium">
        {t('问问这些会议', 'Ask across these meetings')}
      </summary>
      <p className="mt-3 text-xs leading-6 text-muted-foreground">
        {t(
          '在当前筛选范围内查找相关原文，带来源回答。使用会议默认 AI 服务。',
          'Find relevant excerpts within the current filters and answer with sources using the default meeting AI service.',
        )}
      </p>
      <div className="my-3 flex gap-3">
        <Input
          maxLength={200}
          aria-label={t('跨会议问题', 'Question across meetings')}
          value={question}
          onChange={(e) => setQuestion(e.target.value)}
          placeholder={t('例如：预算后来为什么改了？', 'For example: Why did the budget change?')}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void ask()
          }}
        />
        <Button disabled={pending || !question.trim()} onClick={() => void ask()}>
          {pending ? t('检索中…', 'Searching…') : t('查找答案', 'Find answer')}
        </Button>
      </div>
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      )}
      {result && (
        <div className="space-y-3">
          <p className="text-xs text-muted-foreground">
            {t('这次问题：', 'Question: ')}
            {answeredQuestion}
          </p>
          <InsightResult insight={result} open={open} />
        </div>
      )}
    </details>
  )
}
