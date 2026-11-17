import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
import { toast } from 'sonner'
import { useText } from '@/lib/app-state'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Checkbox } from '@/components/ui/checkbox'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { meetingCall, meetingError } from './meetings-api'
import { meetingMessage } from './meeting-messages'
import { FollowupEditor } from './meeting-followup-editor'
import {
  compareTasks,
  dueState,
  inScope,
  taskInfo,
  taskDate,
  type FollowupScope,
  type FollowupTask,
} from './meeting-followup-model'

export function AcrossMeetingTasks({
  version,
  name,
  open,
  identityControl,
}: {
  version: string
  name: string
  open: (id: string, segmentId?: string | null) => void
  identityControl?: ReactNode
}) {
  const t = useText()
  const [tasks, setTasks] = useState<FollowupTask[]>([])
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(true)
  const [chosenScope, setChosenScope] = useState<FollowupScope | null>(null)
  const [completed, setCompleted] = useState(false)
  const [query, setQuery] = useState('')
  const [pending, setPending] = useState(false)
  const [project, setProject] = useState('all')
  const [owner, setOwner] = useState('all')
  const [due, setDue] = useState('all')
  const [editing, setEditing] = useState<FollowupTask | null>(null)
  const [now, setNow] = useState(() => new Date())
  const generation = useRef(0)
  const working = useRef(false)
  const scope =
    !name.trim() && (chosenScope === 'mine' || chosenScope === 'owed')
      ? 'all'
      : chosenScope || (name.trim() ? 'mine' : 'all')
  const load = useCallback(async () => {
    const request = ++generation.current
    const next = await meetingCall<FollowupTask[]>('meeting_tasks')
    if (request === generation.current) {
      setTasks(next)
      setError('')
      setLoading(false)
    }
  }, [])
  useEffect(() => {
    let live = true
    void load().catch((failure) => {
      if (!live) return
      setError(meetingError(failure))
      setLoading(false)
    })
    return () => {
      live = false
      ++generation.current
    }
  }, [version, load])
  useEffect(() => {
    const timer = setInterval(() => setNow(new Date()), 60000)
    return () => clearInterval(timer)
  }, [])
  const counts = (value: FollowupScope) =>
    tasks.filter((task) => !task.item.done && inScope(task, value, name)).length
  const base = tasks.filter(
    (task) =>
      (completed || !task.item.done) &&
      inScope(task, scope, name) &&
      (project === 'all' || task.project === project) &&
      (owner === 'all' || task.item.owner === owner) &&
      `${task.item.text} ${task.item.owner || ''} ${task.item.recipient || ''} ${task.project || ''} ${task.title}`
        .toLowerCase()
        .includes(query.trim().toLowerCase()),
  )
  const filtered = base
    .filter((task) => {
      const state = dueState(task.item.due, now)
      return (
        due === 'all' || (due === 'week' ? state === 'week' || state === 'today' : state === due)
      )
    })
    .sort(compareTasks)
  const eligible = filtered.filter(
    (task) =>
      !task.item.done &&
      taskInfo(task, name).confirmed &&
      task.item.owner?.trim() &&
      taskDate(task.item.due),
  )
  const hasFilters = !!query || project !== 'all' || owner !== 'all' || due !== 'all'
  const resetFilters = () => {
    setQuery('')
    setProject('all')
    setOwner('all')
    setDue('all')
  }
  const action = async (work: () => Promise<void>) => {
    if (working.current) return
    working.current = true
    setPending(true)
    setError('')
    try {
      await work()
    } catch (failure) {
      setError(meetingError(failure))
      void load().catch(() => {})
    } finally {
      working.current = false
      setPending(false)
    }
  }
  const exportTasks = (format: string) =>
    void action(async () => {
      const selected = format === 'ics' ? eligible : filtered
      const saved = await meetingCall<boolean>('meeting_tasks_export', {
        format,
        tasks: selected.map((task) => ({
          meetingId: task.meetingId,
          revision: task.revision,
          index: task.index,
        })),
      })
      if (saved)
        toast.success(
          format === 'ics'
            ? t(
                '提醒文件已导出，导入日历后生效',
                'Reminder file exported; import it into your calendar',
              )
            : t('跟进清单已导出', 'Follow-up list exported'),
        )
    })
  const labels = {
    mine: t('我要做', 'My tasks'),
    owed: t('等别人交付', 'Waiting for others'),
    review: t('需要确认', 'Needs checking'),
    all: t('全部事项', 'All items'),
  }
  const dueLabels = {
    overdue: t('已逾期', 'Overdue'),
    today: t('今天到期', 'Due today'),
    week: t('七天内到期', 'Due this week'),
    later: t('后续到期', 'Due later'),
    unknown: t('日期待补充', 'Date to confirm'),
  }
  return (
    <section className="mt-5 space-y-5" aria-label={t('会议跟进清单', 'Meeting follow-ups')}>
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <h2 className="text-lg font-medium">
            {t('把会议里的承诺跟进到底', 'Follow through on meeting commitments')}
          </h2>
          <p className="mt-2 text-sm leading-6 text-muted-foreground">
            {name.trim()
              ? t(
                  `当前身份：${name.trim()}。根据原文明示的负责人和交付对象分类。`,
                  `Your identity: ${name.trim()}. Categories follow the explicit owner and recipient.`,
                )
              : t(
                  '先设置会议中使用的姓名，才能区分你要做的事和交付给你的事。',
                  'Set your name as it appears in meetings to see your tasks and commitments owed to you.',
                )}
          </p>
        </div>
        {identityControl}
      </div>
      <Tabs
        value={scope}
        onValueChange={(value) => {
          setChosenScope(value as FollowupScope)
          resetFilters()
        }}
      >
        <TabsList
          variant="line"
          className="h-auto flex-wrap justify-start"
          aria-label={t('跟进范围', 'Follow-up scope')}
        >
          {(['mine', 'owed', 'review', 'all'] as const).map((value) => (
            <TabsTrigger
              key={value}
              value={value}
              disabled={(value === 'mine' || value === 'owed') && !name.trim()}
            >
              {labels[value]}{' '}
              <span className="ml-1 text-xs tabular-nums text-muted-foreground">
                {counts(value)}
              </span>
            </TabsTrigger>
          ))}
        </TabsList>
      </Tabs>
      <p className="text-xs leading-6 text-muted-foreground">
        {scope === 'mine'
          ? t(
              '只列负责人明确是你、且有原文支持的承诺。',
              'Only source-backed commitments explicitly assigned to you.',
            )
          : scope === 'owed'
            ? t(
                '只列明确承诺交付给你的事项，其他人的普通待办不会归入这里。',
                'Only explicit commitments to deliver to you appear here.',
              )
            : scope === 'review'
              ? t(
                  '原文依据、负责人或具体日期还需要核对。点击“补充 / 核对”即可处理；已明确归属但缺日期的事项也会显示在这里。',
                  'Check missing evidence, owners or dates using “Edit / check”. Assigned tasks with missing calendar dates also appear here.',
                )
              : t(
                  '汇总所有会议的跟进事项；不确定的内容会明确标记。',
                  'Follow-ups from every meeting, with uncertain items clearly flagged.',
                )}
      </p>
      <div className="flex flex-wrap items-center gap-2 border-y py-3">
        {(['overdue', 'today', 'week', 'unknown'] as const).map((value) => (
          <Button
            key={value}
            size="sm"
            variant={due === value ? 'secondary' : 'ghost'}
            aria-pressed={due === value}
            onClick={() => setDue(due === value ? 'all' : value)}
          >
            {dueLabels[value]} ·{' '}
            {
              base.filter(
                (task) =>
                  !task.item.done &&
                  (value === 'week'
                    ? ['today', 'week'].includes(dueState(task.item.due, now))
                    : dueState(task.item.due, now) === value),
              ).length
            }
          </Button>
        ))}
      </div>
      <div className="flex flex-wrap items-center gap-3">
        <Input
          className="max-w-xs"
          aria-label={t('搜索跟进事项', 'Search follow-ups')}
          placeholder={t('搜索事项、人物、项目或会议…', 'Search task, person, project or meeting…')}
          maxLength={200}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <Select value={project} onValueChange={setProject}>
          <SelectTrigger className="w-36" aria-label={t('待办项目', 'Task project')}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">{t('所有项目', 'All projects')}</SelectItem>
            {[
              ...new Set(
                tasks.map((task) => task.project).filter((value): value is string => !!value),
              ),
            ].map((value) => (
              <SelectItem key={value} value={value}>
                {value}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select value={owner} onValueChange={setOwner}>
          <SelectTrigger className="w-36" aria-label={t('待办负责人', 'Task owner')}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">{t('所有负责人', 'All owners')}</SelectItem>
            {[
              ...new Set(
                tasks.map((task) => task.item.owner).filter((value): value is string => !!value),
              ),
            ].map((value) => (
              <SelectItem key={value} value={value}>
                {value}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {hasFilters && (
          <Button size="sm" variant="ghost" onClick={resetFilters}>
            {t('清除筛选', 'Clear filters')}
          </Button>
        )}
        <label className="flex items-center gap-2 text-xs">
          <Checkbox checked={completed} onCheckedChange={(value) => setCompleted(value === true)} />
          {t('显示已完成', 'Show completed')}
        </label>
      </div>
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {meetingMessage(error, t)}
        </p>
      )}
      {loading ? (
        <p role="status" className="py-5 text-sm text-muted-foreground">
          {t('正在汇总会议事项…', 'Loading meeting follow-ups…')}
        </p>
      ) : (
        <div className="divide-y">
          {filtered.map((task) => {
            const info = taskInfo(task, name),
              state = dueState(task.item.due, now)
            const reasons = [
              !info.confirmed && t('需核对原文', 'Check source'),
              !task.item.owner?.trim() && t('负责人待补充', 'Owner missing'),
              state === 'unknown' && task.item.due && t('日期待补充', 'Date to confirm'),
            ].filter(Boolean)
            return (
              <div key={`${task.meetingId}-${task.index}`} className="flex items-start gap-3 py-5">
                <Checkbox
                  className="mt-1.5"
                  checked={task.item.done}
                  disabled={
                    pending ||
                    info.busy ||
                    (!task.item.done && (!info.confirmed || !task.item.owner?.trim()))
                  }
                  aria-label={
                    scope === 'owed'
                      ? t(`确认已交付：${task.item.text}`, `Mark delivered: ${task.item.text}`)
                      : t(`完成：${task.item.text}`, `Complete: ${task.item.text}`)
                  }
                  onCheckedChange={(value) =>
                    void action(async () => {
                      await meetingCall('meeting_task_toggle', {
                        id: task.meetingId,
                        revision: task.revision,
                        index: task.index,
                        done: value === true,
                      })
                      await load()
                      toast.success(
                        value === true
                          ? t(
                              '已标记完成，来源会议已同步',
                              'Marked complete and synced to the source meeting',
                            )
                          : t('已恢复为未完成', 'Reopened task'),
                      )
                    })
                  }
                />
                <div className="min-w-0 flex-1 space-y-2">
                  <p
                    className={`text-sm leading-7 ${task.item.done ? 'text-muted-foreground line-through' : ''}`}
                  >
                    {task.item.text}
                  </p>
                  <p className="text-xs leading-6 text-muted-foreground">
                    {t('负责人：', 'Owner: ')}
                    {task.item.owner || t('待确认', 'Unknown')}
                    {task.item.recipient && (
                      <>
                        {' '}
                        · {t('交付给：', 'Recipient: ')}
                        {task.item.recipient}
                      </>
                    )}{' '}
                    ·{' '}
                    <span
                      className={!task.item.done && state === 'overdue' ? 'text-destructive' : ''}
                    >
                      {state === 'unknown'
                        ? task.item.due || dueLabels.unknown
                        : `${dueLabels[state]} · ${task.item.due}`}
                    </span>
                  </p>
                  {reasons.length > 0 && !task.item.done && (
                    <p className="text-xs leading-5 text-muted-foreground">{reasons.join(' · ')}</p>
                  )}
                  <button
                    className="text-left text-xs text-muted-foreground underline underline-offset-4 hover:text-foreground"
                    onClick={() => open(task.meetingId, task.item.evidence[0])}
                  >
                    {task.project && `${task.project} · `}
                    {task.title} · {new Date(task.createdAt).toLocaleDateString(t('zh-CN', 'en'))} ·{' '}
                    {t('查看原文', 'View source')}
                  </button>
                  {info.busy && (
                    <p className="text-xs text-muted-foreground">
                      {t(
                        '这场会议正在录音或整理，完成后可修改。',
                        'This meeting is recording or processing. Edit once it finishes.',
                      )}
                    </p>
                  )}
                </div>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={pending || info.busy}
                  onClick={() => setEditing(structuredClone(task))}
                >
                  {t('补充 / 核对', 'Edit / check')}
                </Button>
              </div>
            )
          })}
          {!filtered.length && (
            <div className="space-y-3 py-8">
              <p className="text-sm font-medium">
                {hasFilters
                  ? t('没有符合筛选条件的事项', 'No items match these filters')
                  : scope === 'mine'
                    ? t('暂无明确分配给你的未完成事项', 'No confirmed open tasks assigned to you')
                    : scope === 'owed'
                      ? t(
                          '暂无明确交付给你的未完成事项',
                          'No confirmed open commitments owed to you',
                        )
                      : scope === 'review'
                        ? t('目前没有需要补充或核对的事项', 'No items need checking right now')
                        : t('还没有会议待办', 'No meeting tasks yet')}
              </p>
              <p className="text-xs leading-6 text-muted-foreground">
                {hasFilters
                  ? t(
                      '清除筛选后查看这个范围的全部事项。',
                      'Clear filters to see everything in this scope.',
                    )
                  : t(
                      '会议纪要中出现明确的任务或承诺后，会自动汇总到这里。',
                      'Tasks and commitments in your meeting notes appear here automatically.',
                    )}
              </p>
              {hasFilters ? (
                <Button size="sm" variant="outline" onClick={resetFilters}>
                  {t('清除筛选', 'Clear filters')}
                </Button>
              ) : (
                scope !== 'review' &&
                counts('review') > 0 && (
                  <Button size="sm" variant="outline" onClick={() => setChosenScope('review')}>
                    {t(
                      `查看 ${counts('review')} 条待确认事项`,
                      `Check ${counts('review')} pending items`,
                    )}
                  </Button>
                )
              )}
            </div>
          )}
        </div>
      )}
      <details className="border-t pt-4">
        <summary className="cursor-pointer text-xs text-muted-foreground">
          {t('导出清单与日历提醒', 'Export list or calendar reminders')}
        </summary>
        <div className="mt-3 flex flex-wrap gap-3">
          <Button
            variant="outline"
            size="sm"
            disabled={pending || !filtered.length}
            onClick={() => exportTasks('csv')}
          >
            {t('导出当前清单', 'Export current list')}
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled={pending || !eligible.length}
            onClick={() => exportTasks('ics')}
          >
            {t(
              `导出 ${eligible.length} 条日历提醒`,
              `Export ${eligible.length} calendar reminders`,
            )}
          </Button>
        </div>
        <p className="mt-3 text-xs leading-6 text-muted-foreground">
          {t(
            '提醒仅包含有原文支持、负责人明确且有具体日期的未完成事项。导入日历后生效，默认当天 09:00；不确定的日期需先补充。',
            'Reminders require a source-backed open task, owner and calendar date. Import the file into your calendar; reminders default to 09:00 on the due date.',
          )}
        </p>
      </details>
      {editing && (
        <FollowupEditor
          task={editing}
          onClose={() => setEditing(null)}
          onSaved={async () => {
            await load()
            toast.success(
              t('已更新待办，来源会议已同步', 'Task updated and synced to the source meeting'),
            )
          }}
          openSource={() => {
            setEditing(null)
            open(editing.meetingId, editing.item.evidence[0])
          }}
        />
      )}
    </section>
  )
}
