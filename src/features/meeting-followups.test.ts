import { describe, expect, it } from 'vitest'
import {
  compareTasks,
  dueState,
  inScope,
  taskDate,
  taskInfo,
  type FollowupTask,
} from './meeting-followup-model'

const task = (patch: Partial<FollowupTask['item']> = {}, stale = false): FollowupTask => ({
  meetingId: 'meeting',
  title: '项目周会',
  createdAt: 1,
  revision: 1,
  index: 0,
  summaryStale: stale,
  item: {
    kind: 'todo',
    text: '发送报价',
    owner: '李明',
    due: '2026-10-05',
    recipient: '陈雪',
    evidence: ['s1'],
    done: false,
    support: 'supported',
    status: 'commitment',
    ...patch,
  },
})
describe('personal follow-up classification', () => {
  it('requires explicit, exact owners and recipients instead of assigning every other person’s work to me', () => {
    expect(taskInfo(task(), ' 李明 ').mine).toBe(true)
    expect(taskInfo(task({ owner: '李明明' }), '李明').mine).toBe(false)
    expect(taskInfo(task({ owner: '陈雪', recipient: '李明' }), '李明').owed).toBe(true)
    expect(taskInfo(task({ owner: '陈雪', recipient: null }), '李明').owed).toBe(false)
    expect(taskInfo(task({ owner: null, recipient: '李明' }), '李明').owed).toBe(false)
    expect(taskInfo(task({ owner: '李明', recipient: '李明' }), '李明').owed).toBe(false)
    expect(taskInfo(task(), '').mine).toBe(false)
  })
  it('keeps unsupported, legacy, proposed and outdated claims out of personal commitments', () => {
    for (const value of [
      task({ support: 'review' }),
      task({ support: '' }),
      task({ status: 'proposal' }),
      task({ status: 'pending' }),
      task({}, true),
    ]) {
      expect(taskInfo(value, '李明').mine).toBe(false)
      expect(inScope(value, 'review', '李明')).toBe(true)
    }
    expect(taskInfo(task({ reviewed: true }, true), '李明').mine).toBe(true)
  })
  it('keeps assigned commitments visible while separately asking for a calendar date', () => {
    expect(taskInfo(task({ due: '周五前' }), '李明').mine).toBe(true)
    expect(taskInfo(task({ due: '周五前' }), '李明').review).toBe(true)
    expect(taskInfo(task({ owner: null }), '李明').review).toBe(true)
  })
})
describe('follow-up due dates', () => {
  it('validates real calendar dates without guessing relative or missing dates', () => {
    expect(taskDate('2024-02-29')).not.toBeNull()
    for (const value of ['2026-02-29', '2026-02-30', '2026-13-01', '2026-10-00', '周五前', null])
      expect(taskDate(value)).toBeNull()
  })
  it('uses local calendar days so a task due today is not overdue at midnight', () => {
    const now = new Date(2026, 9, 5, 18)
    expect(dueState('2026-10-04', now)).toBe('overdue')
    expect(dueState('2026-10-05', now)).toBe('today')
    expect(dueState('2026-10-11', now)).toBe('week')
    expect(dueState('2026-10-12', now)).toBe('later')
    expect(dueState('周五', now)).toBe('unknown')
  })
  it('sorts open dated tasks before undated and completed tasks', () => {
    const early = task({ due: '2026-10-01' }),
      later = task({ due: '2026-10-08' }),
      unknown = task({ due: '周五' }),
      done = task({ done: true, due: '2026-09-01' })
    expect([unknown, done, later, early].sort(compareTasks)).toEqual([early, later, unknown, done])
  })
})
