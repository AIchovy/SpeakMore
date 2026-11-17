import { ExperienceShortcuts } from './caption-tools'
import { useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import { useText } from '@/lib/app-state'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
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
  meetingError,
  timecode,
  type Meeting,
  type MeetingItem,
  itemStatus,
} from './meetings-api'

export interface Preferences {
  myName: string
  defaultTemplate: string
  terms: { heard: string; replacement: string }[]
  templateRules?: Record<string, string>
  experienceShortcuts?: boolean
}
export const templates = [
  ['general', '通用会议', 'General'],
  ['customer', '客户沟通', 'Customer conversation'],
  ['review', '产品评审', 'Product review'],
  ['weekly', '项目周会', 'Weekly meeting'],
  ['interview', '面试', 'Interview'],
  ['learning', '课程学习', 'Learning'],
  ['one_on_one', '一对一', 'One-on-one'],
]
export function TemplateChoice({
  value,
  onChange,
  disabled = false,
  stacked = false,
}: {
  value: string
  onChange: (v: string) => void
  disabled?: boolean
  stacked?: boolean
}) {
  const t = useText()
  return (
    <label className={stacked ? 'grid gap-3 text-sm' : 'flex items-center gap-3 text-sm'}>
      <span>{t('成果模板', 'Result template')}</span>
      <Select value={value || 'general'} disabled={disabled} onValueChange={onChange}>
        <SelectTrigger
          aria-label={t('成果模板', 'Result template')}
          className={stacked ? 'w-full' : 'w-[210px]'}
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {templates.map(([id, zh, en]) => (
            <SelectItem key={id} value={id}>
              {t(zh, en)}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </label>
  )
}
export function WorkspacePreferences({
  onSaved,
  termsOnly = false,
  triggerLabel,
}: {
  onSaved?: (p: Preferences) => void
  termsOnly?: boolean
  triggerLabel?: string
}) {
  const t = useText(),
    [open, setOpen] = useState(false),
    [p, setP] = useState<Preferences>({ myName: '', defaultTemplate: 'general', terms: [] }),
    [error, setError] = useState(''),
    [pending, setPending] = useState(false)
  const [heard, setHeard] = useState(''),
    [replacement, setReplacement] = useState('')
  const load = async () => {
    try {
      setP(await meetingCall('meeting_preferences'))
      setError('')
      setOpen(true)
    } catch (e) {
      setError(meetingError(e))
    }
  }
  const save = async () => {
    setPending(true)
    setError('')
    try {
      await meetingCall('meeting_preferences_save', { preferences: p })
      onSaved?.(p)
      setOpen(false)
    } catch (e) {
      setError(meetingError(e))
    } finally {
      setPending(false)
    }
  }
  return (
    <>
      <Button size="sm" variant="outline" onClick={() => void load()}>
        {triggerLabel ||
          (termsOnly
            ? t('术语纠错', 'Terminology corrections')
            : t('我的身份与术语', 'My identity and terminology'))}
      </Button>
      {error && !open && (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      )}
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent closeLabel={t('关闭', 'Close')}>
          <DialogHeader>
            <DialogTitle>
              {termsOnly
                ? t('字幕术语纠错', 'Caption terminology corrections')
                : t('我的身份与术语', 'My identity and terminology')}
            </DialogTitle>
            <DialogDescription>
              {termsOnly
                ? t(
                    '修正常被识别错的人名、产品名或专业词。保存后用于后续字幕和会议转写。',
                    'Correct names, product names or specialist terms that are often misrecognized. Saved corrections apply to future captions and meeting transcripts.',
                  )
                : t(
                    '姓名用于筛选明确分配给你的待办。术语修正会持续用于新转写和新字幕；原有记录保留。',
                    'Your name filters explicitly assigned tasks. Terminology corrections apply to new transcripts and captions; existing records are preserved.',
                  )}
            </DialogDescription>
          </DialogHeader>
          {!termsOnly && (
            <>
              <label className="grid gap-3 text-sm">
                <span>{t('纪要中的我的姓名', 'My name in meeting notes')}</span>
                <Input
                  value={p.myName}
                  maxLength={120}
                  onChange={(e) => setP({ ...p, myName: e.target.value })}
                />
              </label>
              <TemplateChoice
                stacked
                value={p.defaultTemplate}
                onChange={(defaultTemplate) => setP({ ...p, defaultTemplate })}
              />
            </>
          )}
          <ExperienceShortcuts t={t} />
          <div className="grid grid-cols-2 gap-2">
            <Input
              aria-label={t('常被识别为', 'Often recognized as')}
              placeholder={t('常被识别为', 'Often recognized as')}
              value={heard}
              maxLength={120}
              onChange={(e) => setHeard(e.target.value)}
            />
            <Input
              aria-label={t('正确术语', 'Correct term')}
              placeholder={t('正确术语', 'Correct term')}
              value={replacement}
              maxLength={120}
              onChange={(e) => setReplacement(e.target.value)}
            />
          </div>
          <Button
            variant="outline"
            disabled={!heard.trim() || !replacement.trim() || p.terms.length >= 300}
            onClick={() => {
              setP({
                ...p,
                terms: [
                  ...p.terms.filter((v) => v.heard !== heard.trim()),
                  { heard: heard.trim(), replacement: replacement.trim() },
                ],
              })
              setHeard('')
              setReplacement('')
            }}
          >
            {t('加入术语修正', 'Add correction')}
          </Button>
          <div className="max-h-48 overflow-y-auto divide-y">
            {p.terms.map((term) => (
              <div className="flex items-center justify-between py-2 text-sm" key={term.heard}>
                <span>
                  {term.heard} → {term.replacement}
                </span>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => setP({ ...p, terms: p.terms.filter((v) => v !== term) })}
                >
                  {t('移除', 'Remove')}
                </Button>
              </div>
            ))}
          </div>
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
          <DialogFooter>
            <Button disabled={pending} onClick={() => void save()}>
              {t('保存', 'Save')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
export function personalItems(items: MeetingItem[], name: string) {
  return name.trim()
    ? items.filter(
        (i) =>
          i.kind === 'todo' &&
          i.owner?.trim().toLocaleLowerCase() === name.trim().toLocaleLowerCase(),
      )
    : []
}
export function PersonalResult({
  meeting,
  name,
  onEvidence,
}: {
  meeting: Meeting
  name: string
  onEvidence: (id: string) => void
}) {
  const t = useText(),
    [error, setError] = useState('')
  const knowledge = ['learning', 'interview'].includes(meeting.template)
  const confirmed = (i: MeetingItem) =>
    i.support !== 'review' && !['proposal', 'pending'].includes(itemStatus(i))
  const mine = personalItems(meeting.items, name).filter(confirmed)
  const owed = meeting.items.filter(
    (i) =>
      i.kind === 'todo' &&
      confirmed(i) &&
      name.trim() &&
      i.recipient?.trim().toLocaleLowerCase() === name.trim().toLocaleLowerCase() &&
      i.owner?.trim().toLocaleLowerCase() !== name.trim().toLocaleLowerCase(),
  )
  const decisions = meeting.items.filter(
    (i) => confirmed(i) && (i.kind === 'decision' || (knowledge && i.kind === 'fact')),
  )
  const pending = meeting.items.filter(
    (i) =>
      ['proposal', 'pending'].includes(itemStatus(i)) ||
      i.support === 'review' ||
      (i.kind === 'todo' && (!i.owner || !i.due)),
  )
  const groups: [string, MeetingItem[], string][] = [
    [
      knowledge
        ? meeting.template === 'learning'
          ? t('知识与结论', 'Knowledge and findings')
          : t('关键回答与结论', 'Key answers and findings')
        : t('已经决定了什么', 'Confirmed decisions'),
      decisions,
      t('暂无明确内容', 'No confirmed content'),
    ],
    [
      knowledge
        ? t('复习、追问与下一步', 'Review, follow-up and next steps')
        : t('我答应了什么', 'My commitments'),
      knowledge ? [...mine, ...meeting.items.filter((i) => i.kind === 'next')] : mine,
      knowledge
        ? t('暂无明确的后续事项', 'No confirmed next steps')
        : name
          ? t('尚无明确分配给你的任务', 'No tasks explicitly assigned to you yet')
          : t(
              '设置“我的身份”，查看明确分配给你的任务。',
              'Set your identity to see explicitly assigned tasks.',
            ),
    ],
    [
      t('谁还欠我什么', 'Commitments owed to me'),
      owed,
      t(
        '尚无明确承诺交付给你的事项；不根据猜测分配。',
        'No explicit commitments to you found. Ownership is never inferred.',
      ),
    ],
    [
      t('哪些事情还没定', 'Still to confirm'),
      pending,
      t('已整理的内容中未发现待确认事项。', 'No pending items found in the processed content.'),
    ],
  ]
  const result = groups
    .map(
      ([label, items]) =>
        `${label}\n${items.map((i) => `• ${i.done ? t('[已完成] ', '[Done] ') : ''}${i.text}${i.owner ? ` · ${i.owner}` : ''}${i.due ? ` · ${i.due}` : ''}${itemStatus(i) === 'proposal' ? t(' · 提议', ' · Proposal') : itemStatus(i) === 'pending' ? t(' · 待确认', ' · Pending') : ''}${i.support === 'review' ? t(' · 需核对原文', ' · Check against transcript') : ''}`).join('\n')}`,
    )
    .join('\n\n')
  return (
    <section className="my-6 rounded-xl border bg-muted/20 p-5">
      <div className="flex justify-between gap-3">
        <h2 className="text-base font-medium">{t('我的会后成果', 'My meeting results')}</h2>
        <Button
          size="sm"
          variant="outline"
          disabled={!meeting.items.length}
          onClick={() =>
            void meetingCall('captions_copy', {
              text: `${meeting.title}\n\n${meeting.summaryStale ? t('内容已更新，以下结果可能过时。\n\n', 'Content changed; these results may be outdated.\n\n') : ''}${result}`,
            })
              .then(() => {
                setError('')
                toast.success(t('结果卡已复制', 'Result card copied'))
              })
              .catch((e) => setError(meetingError(e)))
          }
        >
          {t('复制结果卡', 'Copy result card')}
        </Button>
      </div>
      {meeting.summaryStale && (
        <p className="mt-2 text-xs text-muted-foreground">
          {t(
            '阶段成果已保留；原文修改或尚未整理的讨论可能影响结果。',
            'Interim results are retained; transcript edits or unprocessed discussion may affect them.',
          )}
        </p>
      )}
      <div className="mt-5 grid gap-6 sm:grid-cols-2">
        {groups.map(([label, items, empty]) => (
          <div key={label}>
            <h3 className="mb-3 text-xs text-muted-foreground">{label}</h3>
            {items.length ? (
              items.map((i, n) => (
                <button
                  key={n}
                  className={`mb-2 block text-left text-sm leading-6 hover:underline ${i.done ? 'line-through text-muted-foreground' : ''}`}
                  onClick={() => i.evidence[0] && onEvidence(i.evidence[0])}
                >
                  {i.text}
                  {(['proposal', 'pending'].includes(itemStatus(i)) || i.support === 'review') && (
                    <span className="block text-xs text-muted-foreground">
                      {itemStatus(i) === 'proposal'
                        ? t('提议', 'Proposal')
                        : itemStatus(i) === 'pending'
                          ? t('待确认', 'Pending')
                          : ''}
                      {i.support === 'review' && t(' · 需核对原文', ' · Check against transcript')}
                    </span>
                  )}
                  {i.kind === 'todo' && (
                    <span className="block text-xs text-muted-foreground">
                      {i.owner || t('负责人待确认', 'Owner to confirm')} ·{' '}
                      {i.due || t('日期待确认', 'Date to confirm')}
                    </span>
                  )}
                </button>
              ))
            ) : (
              <p className="text-sm leading-6 text-muted-foreground">{empty}</p>
            )}
          </div>
        ))}
      </div>
      {error && (
        <p role="alert" className="mt-3 text-sm text-destructive">
          {error}
        </p>
      )}
    </section>
  )
}
export interface SearchHit {
  meetingId: string
  title: string
  createdAt: number
  snippet: string
  segmentId: string | null
  startMs: number | null
}
export function FullTextResults({
  query,
  version,
  open,
  filters,
}: {
  filters?: { project: string; after: string; before: string; speaker: string }
  query: string
  version: string
  open: (id: string, segmentId?: string | null) => void
}) {
  const t = useText(),
    [hits, setHits] = useState<SearchHit[]>([]),
    // The query and filters the shown results belong to; library changes refresh them in place.
    [loadedKey, setLoadedKey] = useState<string | null>(null),
    [error, setError] = useState('')
  const key = JSON.stringify([
    query,
    filters?.project || '',
    filters?.after || '',
    filters?.before || '',
    filters?.speaker || '',
  ])
  const loaded = useRef(loadedKey)
  loaded.current = loadedKey
  useEffect(() => {
    let alive = true
    const timer = setTimeout(
      () =>
        void meetingCall<SearchHit[]>('meeting_search', {
          query,
          project: filters?.project || null,
          after: filters?.after ? new Date(`${filters.after}T00:00:00`).getTime() : null,
          before: filters?.before ? new Date(`${filters.before}T23:59:59.999`).getTime() : null,
          speaker: filters?.speaker || null,
        })
          .then((v) => {
            if (alive) {
              setHits(v)
              setError('')
              setLoadedKey(key)
            }
          })
          .catch((e) => {
            // A failed background refresh keeps the results already shown for this search.
            if (!alive || loaded.current === key) return
            setError(meetingError(e))
            setLoadedKey(key)
          }),
      200,
    )
    return () => {
      alive = false
      clearTimeout(timer)
    }
    // key covers the query and filters; version refreshes results without clearing them.
  }, [key, version])
  const pending = loadedKey !== key
  return (
    <div className="divide-y" aria-live="polite">
      {pending ? (
        <p className="py-5 text-sm text-muted-foreground">
          {t('正在搜索全文…', 'Searching all records…')}
        </p>
      ) : error ? (
        <p role="alert" className="py-5 text-sm text-destructive">
          {error}
        </p>
      ) : hits.length ? (
        <>
          {hits.map((hit, n) => (
            <button
              key={`${hit.meetingId}-${n}`}
              className="w-full rounded-sm p-4 text-left hover:bg-muted/50"
              onClick={() => open(hit.meetingId, hit.segmentId)}
            >
              <p className="text-sm font-medium">
                {hit.title}
                {hit.startMs !== null && (
                  <span className="ml-3 text-xs text-muted-foreground">
                    {timecode(hit.startMs)}
                  </span>
                )}
              </p>
              <p className="mt-2 whitespace-pre-wrap text-sm leading-6 text-muted-foreground">
                {hit.snippet}
              </p>
            </button>
          ))}
          {hits.length === 200 && (
            <p className="py-3 text-xs text-muted-foreground">
              {t(
                '显示前 200 个结果，请缩小关键词范围。',
                'Showing the first 200 results. Refine your query.',
              )}
            </p>
          )}
        </>
      ) : (
        <p className="py-5 text-sm text-muted-foreground">
          {t('没有找到相关原文、纪要或负责人。', 'No matching transcript, note or owner found.')}
        </p>
      )}
    </div>
  )
}
export { AcrossMeetingTasks } from './meeting-followups'

export function SpeakerNames({
  meeting,
  disabled,
  refresh,
}: {
  meeting: Meeting
  disabled: boolean
  refresh: () => Promise<void>
}) {
  const t = useText(),
    [selected, setSelected] = useState(''),
    [name, setName] = useState(''),
    [target, setTarget] = useState(''),
    [pending, setPending] = useState(false),
    [error, setError] = useState('')
  const identities = [
    ...new Map(
      meeting.segments
        .filter((s) => s.speakerId || s.speaker)
        .map((s) => [s.speakerId || s.speaker!, s.speaker || s.speakerId!]),
    ).entries(),
  ]
  if (!identities.length) return null
  return (
    <section className="mb-5 rounded-lg border p-4">
      <h3 className="mb-2 text-sm font-medium">{t('本场说话人', 'Speakers in this meeting')}</h3>
      <p className="mb-3 text-xs leading-5 text-muted-foreground">
        {t(
          '一次改名应用到所有关联片段。短句或归属不确定时标为待确认；可选已有身份进行合并。',
          'Rename all linked segments at once. Short or ambiguous voices are marked for confirmation; you can merge them into an existing identity.',
        )}
      </p>
      <div className="flex flex-wrap items-center gap-2">
        <Select
          value={selected}
          onValueChange={(id) => {
            setSelected(id)
            setName(identities.find((v) => v[0] === id)?.[1] || '')
            setTarget('')
          }}
        >
          <SelectTrigger className="w-52" aria-label={t('选择说话人', 'Select speaker')}>
            <SelectValue placeholder={t('选择说话人', 'Select speaker')} />
          </SelectTrigger>
          <SelectContent>
            {identities.map(([id, label]) => (
              <SelectItem value={id} key={id}>
                {label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Input
          className="w-40"
          aria-label={t('说话人姓名', 'Speaker name')}
          value={name}
          maxLength={80}
          onChange={(e) => setName(e.target.value)}
        />
        <Select
          value={target || 'none'}
          onValueChange={(id) => {
            setTarget(id === 'none' ? '' : id)
            if (id !== 'none') setName(identities.find((v) => v[0] === id)?.[1] || '')
          }}
        >
          <SelectTrigger className="w-44" aria-label={t('合并到已有身份', 'Merge into speaker')}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="none">{t('保留独立身份', 'Keep separate')}</SelectItem>
            {identities
              .filter((v) => v[0] !== selected)
              .map(([id, label]) => (
                <SelectItem value={id} key={id}>
                  {label}
                </SelectItem>
              ))}
          </SelectContent>
        </Select>
        <Button
          size="sm"
          disabled={disabled || pending || !selected || !name.trim()}
          onClick={() => {
            setPending(true)
            setError('')
            void meetingCall('meeting_speaker_rename', {
              id: meeting.id,
              revision: meeting.revision,
              speakerId: selected,
              name,
              mergeWith: target || null,
            })
              .then(refresh)
              .then(() => {
                setSelected('')
                setTarget('')
              })
              .catch((e) => setError(meetingError(e)))
              .finally(() => setPending(false))
          }}
        >
          {t('应用到关联片段', 'Apply to linked segments')}
        </Button>
      </div>
      {error && (
        <p role="alert" className="mt-2 text-sm text-destructive">
          {error}
        </p>
      )}
    </section>
  )
}
