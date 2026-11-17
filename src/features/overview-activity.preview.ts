import type { ActivityDay, ActivityOverview } from './overview-activity-model'
import { dayKey } from './overview-activity-model'
// Explicit development fixture only; this module is excluded from release bundles.
export function activityFixture(day: string | null, empty = false): ActivityOverview {
  if (empty) return { days: [], dictations: [], meetings: [], captions: [] }
  const now = new Date()
  const days: ActivityDay[] = []
  for (let ago = 0; ago < 105; ago++) {
    if (ago % 6 === 3 || ago % 9 === 5) continue
    const date = new Date(now)
    date.setDate(date.getDate() - ago)
    const key = dayKey(date)
    const count = 2 + (ago % 11)
    days.push({
      day: key,
      kind: 'dictation',
      count,
      characters: count * 63,
      durationMs: count * 17000,
      unknownCharacters: 0,
    })
    if (ago % 3 === 0)
      days.push({
        day: key,
        kind: 'meeting',
        count: 1,
        characters: 0,
        durationMs: 2900000 + ago * 10000,
        unknownCharacters: 0,
      })
    if (ago % 4 === 0)
      days.push({
        day: key,
        kind: 'captions',
        count: 1,
        characters: 0,
        durationMs: 1200000,
        unknownCharacters: 0,
      })
  }
  const today = dayKey(now)
  const include = !day || day === today
  return {
    days,
    dictations: include
      ? [
          {
            sessionId: 'a0a0a0a0-1111-4111-8111-a0a0a0a0a0a0',
            rawText: '把刚才那个想法先记下来：客户报价里需要单独列出维护成本，周五前再确认一次。',
            polishedText: null,
            modelId: 'fixture',
            modelSha256: 'a'.repeat(64),
            dictionaryRevision: 1,
            language: 'zh',
            durationMs: 19000,
            asrMs: 1000,
            needsReview: false,
            polishMs: null,
            polishOutcome: 'off',
            deliveryStatus: 'ready_for_copy',
            deliveryReason: null,
            appIdentity: null,
            createdAt: now.getTime() - 180000,
          },
        ]
      : [],
    meetings: include
      ? [
          {
            id: '11111111-1111-4111-8111-111111111111',
            title: '产品评审 · 三小时会议',
            createdAt: now.getTime() - 7200000,
            durationMs: 10800000,
            state: 'ready',
          },
        ]
      : [],
    captions: include
      ? [
          {
            id: 'c0c0c0c0-1111-4111-8111-c0c0c0c0c0c0',
            createdAt: now.getTime() - 3600000,
            durationMs: 1200000,
            mode: 'listen',
            translated: false,
            saved: false,
          },
        ]
      : [],
  }
}
