import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import { isTauri } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import {
  PiCalendarBlank,
  PiMicrophone,
  PiSubtitles,
  PiArrowRight,
  PiArrowClockwise,
} from 'react-icons/pi'
import { call, useApp, useText } from '@/lib/app-state'
import type { ResultRecord } from '@/lib/types'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import {
  defaultVariant,
  deliveryLabel,
  refinementStatus,
  ResultCopy,
  resultPreview,
} from './results'
import {
  activityStreak,
  activityTotal,
  countCharacters,
  dateForDay,
  dayKey,
  heatLevel,
  heatmapDays,
  periodStart,
  recentDictations,
  type ActivityDay,
  type ActivityOverview,
  type ActivityScope,
} from './overview-activity-model'

const heatColors = [
  'bg-muted dark:bg-muted',
  'bg-emerald-100 dark:bg-emerald-950',
  'bg-emerald-300 dark:bg-emerald-800',
  'bg-emerald-500 dark:bg-emerald-600',
  'bg-emerald-700 dark:bg-emerald-400',
]
const noDays: ActivityDay[] = []
type CalendarCell = ReturnType<typeof heatmapDays>[number]
// About 182 cells, each with a tooltip: memoized so that session and result
// updates on the overview do not rebuild them.
const ActivityHeatmap = memo(function ActivityHeatmap({
  calendar,
  counts,
  scopeLabel,
  selectedDay,
  disabled,
  onSelect,
}: {
  calendar: CalendarCell[]
  counts: Map<string, number>
  scopeLabel: string
  selectedDay: string | null
  disabled: boolean
  onSelect: (day: string) => void
}) {
  const t = useText()
  return (
    <div className="overflow-x-auto pb-1">
      <div className="min-w-[440px]">
        <div className="mb-2 ml-7 grid grid-cols-[repeat(26,minmax(0,1fr))] gap-1 text-[10px] text-muted-foreground">
          {Array.from({ length: 26 }, (_, i) => {
            const date = calendar[i * 7].date
            const prev = i ? calendar[(i - 1) * 7].date : null
            return (
              <span key={i}>
                {!prev || date.getMonth() !== prev.getMonth()
                  ? date.toLocaleDateString(t('zh-CN', 'en'), { month: 'short' })
                  : ''}
              </span>
            )
          })}
        </div>
        <div className="flex gap-2">
          <div className="grid w-5 shrink-0 grid-rows-[repeat(7,16px)] items-center text-[10px] text-muted-foreground">
            {[t('一', 'M'), ' ', t('三', 'W'), ' ', t('五', 'F'), ' ', ' '].map((label, i) => (
              <span key={i}>{label}</span>
            ))}
          </div>
          <TooltipProvider delayDuration={150}>
            <div
              className="grid flex-1 grid-flow-col grid-cols-[repeat(26,minmax(0,1fr))] grid-rows-[repeat(7,16px)] gap-1"
              aria-label={t('按日期查看使用记录', 'Browse activity by date')}
            >
              {calendar.map((cell) => {
                const count = counts.get(cell.day) || 0
                const label = t(
                  `${cell.day}，${scopeLabel} ${count} 次`,
                  `${cell.day}: ${count} ${scopeLabel.toLowerCase()} sessions`,
                )
                return (
                  <Tooltip key={cell.day}>
                    <TooltipTrigger asChild>
                      <button
                        type="button"
                        disabled={cell.future || disabled}
                        aria-label={label}
                        aria-pressed={selectedDay === cell.day}
                        onClick={() => onSelect(cell.day)}
                        className={`h-full w-full min-w-0 rounded-[3px] border border-foreground/5 outline-offset-2 focus-visible:outline-2 focus-visible:outline-foreground ${cell.future ? 'opacity-20' : heatColors[heatLevel(count)]} ${selectedDay === cell.day ? 'ring-2 ring-foreground ring-offset-2 ring-offset-background' : ''}`}
                      />
                    </TooltipTrigger>
                    <TooltipContent>{label}</TooltipContent>
                  </Tooltip>
                )
              })}
            </div>
          </TooltipProvider>
        </div>
      </div>
    </div>
  )
})
export function OverviewActivity({
  onResult,
}: {
  onResult: (record: ResultRecord, trigger: HTMLElement) => void
}) {
  const { data } = useApp(),
    t = useText()
  const [activity, setActivity] = useState<ActivityOverview | null>(null)
  const [responseDay, setResponseDay] = useState<string | null | undefined>(undefined)
  const [selectedDay, setSelectedDay] = useState<string | null>(null)
  const [scope, setScope] = useState<ActivityScope>('all')
  const [period, setPeriod] = useState('week')
  const [recentScope, setRecentScope] = useState<ActivityScope>('all')
  const [error, setError] = useState(false)
  const [refresh, setRefresh] = useState(0)
  const [now, setNow] = useState(() => new Date())
  const generation = useRef(0)
  const latestSession = data?.recentResults[0]?.sessionId
  useEffect(() => {
    const request = ++generation.current
    setError(false)
    void call<ActivityOverview>('overview_activity', { day: selectedDay })
      .then((next) => {
        if (request !== generation.current) return
        setActivity(next)
        setResponseDay(selectedDay)
      })
      .catch(() => {
        if (request === generation.current) setError(true)
      })
    return () => {
      ++generation.current
    }
  }, [selectedDay, refresh, latestSession, data?.settings.historyEnabled])
  useEffect(() => {
    let live = true
    const subscriptions: (() => void)[] = []
    let timer: ReturnType<typeof setTimeout> | undefined
    const update = () => {
      if (timer) return
      timer = setTimeout(() => {
        timer = undefined
        if (live) setRefresh((value) => value + 1)
      }, 1000)
    }
    if (isTauri())
      for (const event of [
        'meetings_changed',
        'activity_changed',
        'history_changed',
        'history_delivery_changed',
      ]) {
        void listen(event, update)
          .then((off) => {
            if (live) subscriptions.push(off)
            else off()
          })
          .catch(() => {})
      }
    const resume = () => {
      if (document.visibilityState !== 'hidden') {
        setNow(new Date())
        update()
      }
    }
    window.addEventListener('focus', resume)
    document.addEventListener('visibilitychange', resume)
    const interval = setInterval(() => setNow(new Date()), 60000)
    return () => {
      live = false
      clearTimeout(timer)
      clearInterval(interval)
      subscriptions.forEach((off) => off())
      window.removeEventListener('focus', resume)
      document.removeEventListener('visibilitychange', resume)
    }
  }, [])
  const loaded = activity !== null
  const pending = responseDay !== selectedDay
  const days = activity?.days ?? noDays
  const today = dayKey(now)
  const from = periodStart(period, now)
  const total = (kind: ActivityScope) => activityTotal(days, kind, from, today, selectedDay)
  const dictation = total('dictation'),
    meeting = total('meeting'),
    caption = total('captions')
  const week = activityTotal(days, 'all', periodStart('week', now), today)
  const streak = activityStreak(days, now)
  const locale = t('zh-CN', 'en')
  const nf = useMemo(() => new Intl.NumberFormat(locale), [locale])
  const stamp = useMemo(
    () =>
      new Intl.DateTimeFormat(locale, {
        month: 'short',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit',
      }),
    [locale],
  )
  const toggleDay = useCallback(
    (day: string) => setSelectedDay((current) => (current === day ? null : day)),
    [],
  )
  const duration = (ms: number) => {
    if (ms <= 0) return t('0 分钟', '0 min')
    if (ms < 60000) return t('不到 1 分钟', 'Under 1 min')
    const minutes = Math.floor(ms / 60000)
    const hours = Math.floor(minutes / 60)
    return hours
      ? t(
          `${hours} 小时${minutes % 60 ? ` ${minutes % 60} 分钟` : ''}`,
          `${hours} h${minutes % 60 ? ` ${minutes % 60} min` : ''}`,
        )
      : t(`${minutes} 分钟`, `${minutes} min`)
  }
  const labels: Record<ActivityScope, string> = {
    all: t('全部', 'All'),
    dictation: t('开口听写', 'Dictation'),
    meeting: t('会议', 'Meetings'),
    captions: t('字幕', 'Captions'),
  }
  const calendar = useMemo(() => heatmapDays(now), [now])
  const heatCounts = useMemo(() => {
    const counts = new Map<string, number>()
    for (const day of days)
      if (scope === 'all' || day.kind === scope)
        counts.set(day.day, (counts.get(day.day) || 0) + day.count)
    return counts
  }, [days, scope])
  const transient = data?.recentResults || []
  const dictations = recentDictations(
    pending ? [] : activity?.dictations || [],
    transient,
    selectedDay,
  )
  const recent = [
    ...dictations.map((record) => ({
      kind: 'dictation' as const,
      id: record.sessionId,
      at: record.createdAt,
      record,
    })),
    ...(pending ? [] : activity?.meetings || []).map((record) => ({
      kind: 'meeting' as const,
      id: record.id,
      at: record.createdAt,
      record,
    })),
    ...(pending ? [] : activity?.captions || []).map((record) => ({
      kind: 'captions' as const,
      id: record.id,
      at: record.createdAt,
      record,
    })),
  ]
    .filter((item) => recentScope === 'all' || item.kind === recentScope)
    .sort((a, b) => b.at - a.at || a.id.localeCompare(b.id))
    .slice(0, recentScope === 'all' ? 6 : 5)
  return (
    <section className="mt-6 space-y-5" aria-label={t('我的使用足迹', 'My activity')}>
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <h2 className="text-lg font-medium">{t('你的使用足迹', 'Your activity')}</h2>
          <p className="mt-1.5 text-xs leading-6 text-muted-foreground">
            {error
              ? t('使用记录暂时无法读取。', 'Activity is temporarily unavailable.')
              : !loaded
                ? t('正在读取本地使用记录…', 'Loading local activity…')
                : week.activeDays
                  ? t(
                      `本周已经使用 ${week.activeDays} 天${streak > 1 ? `，连续使用 ${streak} 天` : ''}。`,
                      `Active ${week.activeDays} days this week${streak > 1 ? `, with a ${streak}-day streak` : ''}.`,
                    )
                  : t(
                      '从今天的一次开口开始，留下自己的使用足迹。',
                      'Start with one thought today and build your activity history.',
                    )}
          </p>
        </div>
        <div className="flex flex-wrap gap-1" aria-label={t('统计范围', 'Statistics period')}>
          {(['week', 'month', 'all'] as const).map((value) => (
            <Button
              key={value}
              size="sm"
              variant={period === value && !selectedDay ? 'secondary' : 'ghost'}
              aria-pressed={period === value && !selectedDay}
              onClick={() => {
                setPeriod(value)
                setSelectedDay(null)
              }}
            >
              {value === 'week'
                ? t('本周', 'This week')
                : value === 'month'
                  ? t('近 30 天', 'Last 30 days')
                  : t('全部记录', 'All recorded')}
            </Button>
          ))}
        </div>
      </div>
      {error && (
        <div
          role="alert"
          className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-destructive/30 p-4 text-sm"
        >
          <span>
            {t(
              '使用统计暂时无法读取。可以继续听写，稍后重试。',
              'Activity could not be loaded. You can keep dictating and try again later.',
            )}
          </span>
          <Button size="sm" variant="outline" onClick={() => setRefresh((value) => value + 1)}>
            <PiArrowClockwise aria-hidden />
            {t('重试', 'Retry')}
          </Button>
        </div>
      )}
      {selectedDay && (
        <div className="flex flex-wrap items-center justify-between gap-3 text-sm">
          <span>
            {t('查看这一天：', 'Viewing: ')}
            {dateForDay(selectedDay).toLocaleDateString(t('zh-CN', 'en'), {
              month: 'long',
              day: 'numeric',
              year: 'numeric',
            })}
          </span>
          <Button size="sm" variant="ghost" onClick={() => setSelectedDay(null)}>
            {t('返回最近活动', 'Back to recent activity')}
          </Button>
        </div>
      )}
      <dl className="grid grid-cols-1 gap-6 border-y py-4 sm:grid-cols-3 sm:gap-0">
        <div className="space-y-2 sm:pr-5">
          <dt className="flex items-center gap-2 text-xs text-muted-foreground">
            <PiMicrophone className="size-4" aria-hidden />
            {t('听写字数', 'Dictated characters')}
          </dt>
          <dd className="flex items-baseline gap-1.5 text-3xl font-medium tracking-tight tabular-nums">
            {loaded && !error
              ? dictation.characters === 0 && dictation.unknownCharacters > 0
                ? '—'
                : nf.format(dictation.characters)
              : '—'}
            <span className="text-xs font-normal tracking-normal text-muted-foreground">
              {t('字', 'characters')}
            </span>
          </dd>
          <p className="text-xs leading-5 text-muted-foreground">
            {loaded && !error
              ? t(
                  `${nf.format(dictation.count)} 次开口 · ${duration(dictation.durationMs)}`,
                  `${nf.format(dictation.count)} dictations · ${duration(dictation.durationMs)}`,
                )
              : error
                ? t('暂时不可用', 'Unavailable')
                : t('读取中', 'Loading')}
          </p>
        </div>
        <div className="space-y-2 sm:border-l sm:px-5">
          <dt className="flex items-center gap-2 text-xs text-muted-foreground">
            <PiCalendarBlank className="size-4" aria-hidden />
            {t('会议记录', 'Meetings recorded')}
          </dt>
          <dd className="flex items-baseline gap-1.5 text-3xl font-medium tracking-tight tabular-nums">
            {loaded && !error ? nf.format(meeting.count) : '—'}
            <span className="text-xs font-normal tracking-normal text-muted-foreground">
              {t('场', 'meetings')}
            </span>
          </dd>
          <p className="text-xs leading-5 text-muted-foreground">
            {loaded && !error
              ? duration(meeting.durationMs)
              : error
                ? t('暂时不可用', 'Unavailable')
                : t('读取中', 'Loading')}
          </p>
        </div>
        <div className="space-y-2 sm:border-l sm:pl-5">
          <dt className="flex items-center gap-2 text-xs text-muted-foreground">
            <PiSubtitles className="size-4" aria-hidden />
            {t('字幕陪伴', 'Time with captions')}
          </dt>
          <dd className="text-2xl font-medium tracking-tight tabular-nums">
            {loaded && !error ? duration(caption.durationMs) : '—'}
          </dd>
          <p className="text-xs leading-5 text-muted-foreground">
            {loaded && !error
              ? t(`使用 ${nf.format(caption.count)} 次`, `Used ${nf.format(caption.count)} times`)
              : error
                ? t('暂时不可用', 'Unavailable')
                : t('读取中', 'Loading')}
          </p>
        </div>
      </dl>
      {dictation.unknownCharacters > 0 && !error && (
        <p className="text-xs leading-6 text-muted-foreground">
          {t(
            `${dictation.unknownCharacters} 条旧听写已没有原文，只计次数，未推测字数和时长。`,
            `${dictation.unknownCharacters} older dictations have no remaining transcript. Their character counts and durations are unknown.`,
          )}
        </p>
      )}
      <div className="space-y-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <h3 className="text-sm font-medium">
            {t('近半年的使用热力图', 'Activity over the last six months')}
          </h3>
          <div
            className="flex flex-wrap gap-1"
            aria-label={t('热力图内容', 'Heatmap activity type')}
          >
            {(['all', 'dictation', 'meeting', 'captions'] as const).map((value) => (
              <Button
                key={value}
                variant={scope === value ? 'secondary' : 'ghost'}
                size="sm"
                aria-pressed={scope === value}
                onClick={() => setScope(value)}
              >
                {labels[value]}
              </Button>
            ))}
          </div>
        </div>
        <ActivityHeatmap
          calendar={calendar}
          counts={heatCounts}
          scopeLabel={labels[scope]}
          selectedDay={selectedDay}
          disabled={!loaded || error}
          onSelect={toggleDay}
        />
        <div className="flex flex-wrap items-center justify-between gap-3 text-[11px] text-muted-foreground">
          <p>
            {t(
              '颜色越深，使用次数越多。点击日期查看当天记录。',
              'Darker squares mean more sessions. Select a date to see its records.',
            )}
          </p>
          <div
            className="flex items-center gap-1.5"
            aria-label={t('热力图颜色：由少到多', 'Heatmap legend: fewer to more')}
          >
            <span>{t('少', 'Less')}</span>
            {heatColors.map((color, i) => (
              <span key={i} className={`size-2.5 rounded-[2px] ${color}`} />
            ))}
            <span>{t('多', 'More')}</span>
          </div>
        </div>
      </div>
      <div className="border-t pt-5">
        <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
          <h3 className="text-sm font-medium">
            {selectedDay
              ? t('当天记录', 'Records for this day')
              : t('最近的开口与记录', 'Recent dictation and activity')}
          </h3>
          <div className="flex gap-1" aria-label={t('最近记录类型', 'Recent activity type')}>
            {(['all', 'dictation', 'meeting', 'captions'] as const).map((value) => (
              <Button
                key={value}
                size="sm"
                variant={recentScope === value ? 'secondary' : 'ghost'}
                aria-pressed={recentScope === value}
                onClick={() => setRecentScope(value)}
              >
                {labels[value]}
              </Button>
            ))}
          </div>
        </div>
        {recent.map((item) => {
          const at = stamp.format(item.at)
          if (item.kind === 'dictation')
            return (
              <article key={item.id} className="flex gap-3 border-b px-3 py-4 last:border-0">
                <PiMicrophone className="mt-1 size-4 shrink-0 text-muted-foreground" aria-hidden />
                <div className="min-w-0 flex-1">
                  <div className="mb-2 flex items-center justify-between gap-3">
                    <p className="text-xs text-muted-foreground">
                      {at} ·{' '}
                      {t(
                        `${countCharacters(item.record.rawText)} 字`,
                        `${countCharacters(item.record.rawText)} characters`,
                      )}{' '}
                      · {deliveryLabel(item.record.deliveryStatus, t)}
                    </p>
                    <ResultCopy record={item.record} />
                  </div>
                  <button
                    className="block w-full text-left"
                    aria-label={`${t('查看结果：', 'View result: ')}${resultPreview(item.record)}`}
                    onClick={(e) => onResult(item.record, e.currentTarget)}
                  >
                    <span className="line-clamp-2 whitespace-pre-wrap break-words text-sm leading-7">
                      {defaultVariant(item.record) === 'polished'
                        ? item.record.polishedText
                        : item.record.rawText}
                    </span>
                    <span className="mt-1 inline-flex items-center gap-1 text-xs text-muted-foreground">
                      {t('查看完整文字', 'Read full text')}
                      <PiArrowRight aria-hidden />
                    </span>
                  </button>
                  <p className="mt-2 text-xs text-muted-foreground">
                    {refinementStatus(item.record, t)}
                    {item.record.needsReview && ` · ${t('请检查后使用', 'Review before use')}`}
                  </p>
                </div>
              </article>
            )
          return (
            <Link
              key={item.id}
              to={
                item.kind === 'meeting'
                  ? `/meetings?meeting=${item.id}`
                  : item.record.saved
                    ? `/captions?archive=${item.id}`
                    : '/captions'
              }
              className="flex items-start gap-3 rounded-md border-b px-3 py-4 outline-offset-2 hover:bg-muted/50 focus-visible:outline-2 last:border-0"
            >
              {item.kind === 'meeting' ? (
                <PiCalendarBlank
                  className="mt-1 size-4 shrink-0 text-muted-foreground"
                  aria-hidden
                />
              ) : (
                <PiSubtitles className="mt-1 size-4 shrink-0 text-muted-foreground" aria-hidden />
              )}
              <div className="min-w-0 flex-1">
                <p className="text-sm leading-6">
                  {item.kind === 'meeting' ? item.record.title : t('实时字幕', 'Live captions')}
                </p>
                <p className="mt-1 text-xs leading-6 text-muted-foreground">
                  {at} · {duration(item.record.durationMs)} ·{' '}
                  {item.kind === 'meeting'
                    ? ['recording', 'paused'].includes(item.record.state)
                      ? t('录音中', 'Recording')
                      : ['queued', 'transcribing', 'summarizing'].includes(item.record.state)
                        ? t('整理中', 'Processing')
                        : t('查看会议', 'View meeting')
                    : item.record.saved
                      ? t('回看已保存字幕', 'Read saved captions')
                      : t('内容未保留，仅统计使用', 'Content not retained; usage only')}
                </p>
              </div>
              <PiArrowRight
                className="size-4 shrink-0 self-center text-muted-foreground"
                aria-hidden
              />
            </Link>
          )
        })}
        {pending && !error && (
          <p role="status" className="py-4 text-sm text-muted-foreground">
            {t('正在读取记录…', 'Loading records…')}
          </p>
        )}
        {!recent.length && !pending && !error && (
          <div className="space-y-2 py-5">
            <p className="text-sm">
              {selectedDay
                ? t('这一天没有可回看的记录', 'No records are available for this day')
                : t('从一句话、一场会议开始', 'Start with a sentence or a meeting')}
            </p>
            <p className="text-xs leading-6 text-muted-foreground">
              {t(
                '使用次数会留下；只有你选择保存的内容才能在以后回看。',
                'Session counts remain; only content you choose to save can be read later.',
              )}
            </p>
          </div>
        )}
      </div>
      <details className="text-xs text-muted-foreground">
        <summary className="cursor-pointer">
          {t('这些数字如何计算？', 'How are these numbers calculated?')}
        </summary>
        <p className="mt-3 max-w-3xl leading-6">
          {t(
            '统计在本机保存次数、字数和时长，不保存额外内容。字数按听写原文的文字与数字计算，不含空白和标点，不重复计算 AI 整理结果。会议按现存录音记录统计，续录仍算一场；字幕时长扣除暂停，旧版字幕没有使用统计。热力图按本地开始日期计数，旧数据只恢复仍可读取的部分。',
            'Usage metadata stays on this device. Character counts use letters and numbers in the original dictation, excluding whitespace and punctuation; refinement does not add another session. Meeting counts use retained recordings, including resumed sessions once. Caption time excludes pauses; older caption sessions were not tracked. Dates use local start dates, and only recoverable older activity is included.',
          )}
        </p>
      </details>
    </section>
  )
}
