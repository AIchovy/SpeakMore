import { describe, expect, it } from 'vitest'
import {
  activityStreak,
  activityTotal,
  countCharacters,
  dayKey,
  heatmapDays,
  periodStart,
  recentDictations,
  type ActivityDay,
} from './overview-activity-model'
import type { ResultRecord } from '@/lib/types'

const day = (date: string, kind: ActivityDay['kind'] = 'dictation', count = 1): ActivityDay => ({
  day: date,
  kind,
  count,
  characters: kind === 'dictation' ? 30 : 0,
  durationMs: 60000,
  unknownCharacters: 0,
})
describe('home activity', () => {
  it('counts selected categories and dates without multiplying days used', () => {
    const days = [
      day('2026-10-05'),
      day('2026-10-05', 'meeting'),
      day('2026-10-04', 'captions'),
      day('2026-10-06'),
    ]
    expect(activityTotal(days, 'all', '2026-10-04', '2026-10-05')).toMatchObject({
      count: 3,
      activeDays: 2,
      durationMs: 180000,
      characters: 30,
    })
    expect(activityTotal(days, 'meeting')).toMatchObject({ count: 1, activeDays: 1, characters: 0 })
    expect(activityTotal(days, 'all', '', '9999', '2026-10-05')).toMatchObject({
      count: 2,
      activeDays: 1,
    })
  })
  it('keeps unknown older character counts separate rather than estimating', () => {
    expect(
      activityTotal([{ ...day('2026-10-05'), characters: 0, unknownCharacters: 1 }], 'dictation'),
    ).toMatchObject({ count: 1, characters: 0, unknownCharacters: 1 })
    expect(countCharacters('你好，AI 2026! 👋')).toBe(8)
  })
  it('uses calendar Mondays and a real 30-day window across year boundaries', () => {
    expect(periodStart('week', new Date(2026, 0, 1, 18))).toBe('2025-12-29')
    expect(periodStart('month', new Date(2026, 0, 1, 18))).toBe('2025-12-03')
    expect(periodStart('all', new Date())).toBe('')
  })
  it('starts heatmap columns on Monday and disables future dates', () => {
    const days = heatmapDays(new Date(2026, 9, 5, 0, 1), 2)
    expect(days).toHaveLength(14)
    expect(days[0].date.getDay()).toBe(1)
    expect(days[0].day).toBe('2026-09-28')
    expect(days[7]).toMatchObject({ day: '2026-10-05', future: false })
    expect(days[8]).toMatchObject({ day: '2026-10-06', future: true })
  })
  it('keeps yesterday’s streak until today is over and never bridges a missing day', () => {
    const now = new Date(2026, 9, 5, 20)
    expect(activityStreak([day('2026-10-04'), day('2026-10-03', 'captions')], now)).toBe(2)
    expect(activityStreak([day('2026-10-05'), day('2026-10-03')], now)).toBe(1)
    expect(activityStreak([day('2026-10-03')], now)).toBe(0)
  })
  it('prefers current temporary results, deduplicates saved results and filters by local date', () => {
    const now = new Date(2026, 9, 5, 18)
    const saved = { sessionId: 'same', createdAt: now.getTime(), rawText: '原文' } as ResultRecord
    const updated = { ...saved, rawText: '最新文字' }
    const older = { ...saved, sessionId: 'older', createdAt: now.getTime() - 86400000 }
    expect(recentDictations([saved, older], [updated])).toEqual([updated, older])
    expect(recentDictations([], [older, updated], dayKey(now))).toEqual([updated])
    expect(
      recentDictations(
        [],
        Array.from({ length: 10 }, (_, i) => ({ ...saved, sessionId: String(i) })),
      ),
    ).toHaveLength(5)
  })
})
