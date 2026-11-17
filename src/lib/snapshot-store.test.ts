import { beforeEach, describe, expect, it } from 'vitest'
import { createPreview } from './preview'
import { LiveSessionStore, mergeSnapshot, shareEqual } from './snapshot-store'
import type { ResultRecord, Snapshot } from './types'

let base: Snapshot
const tick = (snapshot: Snapshot, patch: Partial<Snapshot['session']> = {}, audioLevel = 0.04) =>
  ({
    ...structuredClone(snapshot),
    revision: snapshot.revision + 1,
    audioLevel,
    session: { ...structuredClone(snapshot.session), ...patch },
  }) as Snapshot
beforeEach(async () => {
  base = (await createPreview()('get_snapshot')) as Snapshot
  base.session = { ...base.session, sessionId: 's', phase: 'recording', elapsedMs: 1000 }
  base.recentResults = [{ sessionId: 'r', rawText: 'kept' } as ResultRecord]
})

describe('snapshot updates during dictation', () => {
  it('keeps the same snapshot when only the timer, input level and revision advance', () => {
    expect(mergeSnapshot(base, tick(base, { elapsedMs: 1200 }, 0.05))).toBe(base)
  })
  it('replaces changed branches only and carries the newest live values', () => {
    const next = tick(base, { phase: 'transcribing', elapsedMs: 1400 })
    const merged = mergeSnapshot(base, next)
    expect(merged).not.toBe(base)
    expect(merged.session.phase).toBe('transcribing')
    expect(merged.session.elapsedMs).toBe(1400)
    expect(merged.revision).toBe(next.revision)
    expect(merged.audioLevel).toBe(next.audioLevel)
    expect(merged.recentResults).toBe(base.recentResults)
    expect(merged.settings).toBe(base.settings)
    expect(merged.dictionary).toBe(base.dictionary)
  })
  it('detects added results and nested changes', () => {
    const next = tick(base)
    next.recentResults = [{ sessionId: 'n', rawText: 'new' } as ResultRecord, ...next.recentResults]
    const merged = mergeSnapshot(base, next)
    expect(merged.recentResults).toHaveLength(2)
    expect(merged.recentResults[1]).toBe(base.recentResults[0])
    expect(shareEqual({ a: [1, { b: 2 }] }, { a: [1, { b: 3 }] })).toEqual({ a: [1, { b: 3 }] })
    expect(shareEqual({ a: 1 }, { a: 1, b: undefined })).toEqual({ a: 1, b: undefined })
  })
  it('notifies live subscribers only for changed values', () => {
    const store = new LiveSessionStore()
    let calls = 0
    const off = store.subscribe(() => calls++)
    store.set(0, 0)
    store.set(200, 0.03)
    store.set(200, 0.03)
    expect(calls).toBe(1)
    expect(store.snapshot()).toEqual({ elapsedMs: 200, audioLevel: 0.03 })
    off()
    store.set(400, 0.03)
    expect(calls).toBe(1)
  })
})
