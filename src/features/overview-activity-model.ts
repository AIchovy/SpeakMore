import type { ResultRecord } from '@/lib/types'

export type ActivityKind = 'dictation' | 'meeting' | 'captions'
export type ActivityScope = ActivityKind | 'all'
export interface ActivityDay {
  day: string
  kind: ActivityKind
  count: number
  characters: number
  durationMs: number
  unknownCharacters: number
}
export interface RecentMeeting {
  id: string
  title: string
  createdAt: number
  durationMs: number
  state: string
}
export interface CaptionUsage {
  id: string
  createdAt: number
  durationMs: number
  mode: string
  translated: boolean
  saved: boolean
}
export interface ActivityOverview {
  days: ActivityDay[]
  dictations: ResultRecord[]
  meetings: RecentMeeting[]
  captions: CaptionUsage[]
}
export const dayKey = (date: Date) =>
  `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`
export const dateForDay = (day: string) => {
  const [year, month, date] = day.split('-').map(Number)
  const value = new Date(0)
  value.setFullYear(year, month - 1, date)
  value.setHours(12, 0, 0, 0)
  return value
}
export function heatmapDays(now: Date, weeks = 26) {
  const end = new Date(now)
  end.setHours(12, 0, 0, 0)
  const monday = (end.getDay() + 6) % 7
  const start = new Date(end)
  start.setDate(start.getDate() - monday - (weeks - 1) * 7)
  return Array.from({ length: weeks * 7 }, (_, i) => {
    const date = new Date(start)
    date.setDate(date.getDate() + i)
    return { day: dayKey(date), date, future: dayKey(date) > dayKey(end) }
  })
}
export function activityTotal(
  days: ActivityDay[],
  scope: ActivityScope,
  from = '',
  to = '9999',
  selected?: string | null,
) {
  const filtered = days.filter(
    (d) =>
      (scope === 'all' || d.kind === scope) &&
      (selected ? d.day === selected : d.day >= from && d.day <= to),
  )
  return {
    count: filtered.reduce((sum, d) => sum + d.count, 0),
    characters: filtered.reduce((sum, d) => sum + d.characters, 0),
    durationMs: filtered.reduce((sum, d) => sum + d.durationMs, 0),
    unknownCharacters: filtered.reduce((sum, d) => sum + d.unknownCharacters, 0),
    activeDays: new Set(filtered.filter((d) => d.count > 0).map((d) => d.day)).size,
  }
}
export function activityStreak(days: ActivityDay[], now: Date, scope: ActivityScope = 'all') {
  const active = new Set(
    days.filter((d) => d.count > 0 && (scope === 'all' || d.kind === scope)).map((d) => d.day),
  )
  const date = new Date(now)
  date.setHours(12, 0, 0, 0)
  if (!active.has(dayKey(date))) date.setDate(date.getDate() - 1)
  let count = 0
  while (active.has(dayKey(date))) {
    count++
    date.setDate(date.getDate() - 1)
  }
  return count
}
export function periodStart(period: string, now: Date) {
  if (period === 'all') return ''
  const date = new Date(now)
  date.setDate(date.getDate() - (period === 'week' ? (date.getDay() + 6) % 7 : 29))
  return dayKey(date)
}
export const heatLevel = (count: number) =>
  count <= 0 ? 0 : count <= 2 ? 1 : count <= 5 ? 2 : count <= 10 ? 3 : 4
export const countCharacters = (text: string) => (text.match(/[\p{L}\p{N}]/gu) || []).length
export function recentDictations(
  saved: ResultRecord[],
  temporary: ResultRecord[],
  selected?: string | null,
) {
  const map = new Map(saved.map((record) => [record.sessionId, record]))
  for (const record of temporary)
    if (!selected || dayKey(new Date(record.createdAt)) === selected)
      map.set(record.sessionId, record)
  return [...map.values()]
    .sort((a, b) => b.createdAt - a.createdAt || a.sessionId.localeCompare(b.sessionId))
    .slice(0, 5)
}
