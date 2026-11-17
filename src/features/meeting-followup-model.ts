import { itemStatus, type MeetingItem } from './meetings-api'

export interface FollowupTask {
  meetingId: string
  title: string
  project?: string
  createdAt: number
  revision: number
  index: number
  item: MeetingItem
  summaryStale?: boolean
  state?: string
}
export type FollowupScope = 'mine' | 'owed' | 'review' | 'all'
export type DueState = 'overdue' | 'today' | 'week' | 'later' | 'unknown'
export const samePerson = (a: string | null | undefined, b: string) =>
  !!a?.trim() && !!b.trim() && a.trim().toLocaleLowerCase() === b.trim().toLocaleLowerCase()

export function taskDate(value: string | null | undefined): Date | null {
  if (!value || !/^\d{4}-\d{2}-\d{2}$/.test(value)) return null
  const [year, month, day] = value.split('-').map(Number)
  const date = new Date(0)
  date.setFullYear(year, month - 1, day)
  date.setHours(0, 0, 0, 0)
  return date.getFullYear() === year && date.getMonth() === month - 1 && date.getDate() === day
    ? date
    : null
}
export function dueState(value: string | null | undefined, now = new Date()): DueState {
  const date = taskDate(value)
  if (!date) return 'unknown'
  const today = new Date(now)
  today.setHours(0, 0, 0, 0)
  const week = new Date(today)
  week.setDate(week.getDate() + 7)
  return date < today
    ? 'overdue'
    : date.getTime() === today.getTime()
      ? 'today'
      : date < week
        ? 'week'
        : 'later'
}
export function taskInfo(task: FollowupTask, name: string) {
  const confirmed =
    itemStatus(task.item) === 'commitment' &&
    task.item.support === 'supported' &&
    (!task.summaryStale || task.item.reviewed === true)
  const owned = !!task.item.owner?.trim()
  return {
    confirmed,
    mine: confirmed && samePerson(task.item.owner, name),
    owed:
      confirmed &&
      owned &&
      !samePerson(task.item.owner, name) &&
      samePerson(task.item.recipient, name),
    review: !confirmed || !owned || !taskDate(task.item.due),
    busy: ['preparing', 'recording', 'paused', 'transcribing', 'summarizing'].includes(
      task.state || '',
    ),
  }
}
export function inScope(task: FollowupTask, scope: FollowupScope, name: string) {
  return scope === 'all' || taskInfo(task, name)[scope]
}
export function compareTasks(a: FollowupTask, b: FollowupTask) {
  return (
    Number(a.item.done) - Number(b.item.done) ||
    (taskDate(a.item.due)?.getTime() ?? Infinity) - (taskDate(b.item.due)?.getTime() ?? Infinity) ||
    b.createdAt - a.createdAt ||
    a.index - b.index
  )
}
