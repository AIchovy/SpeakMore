import { describe, expect, it } from 'vitest'
import { HistoryLoader } from './history-loader'
import type { HistoryPage, ResultRecord } from './types'
const record = (sessionId: string) => ({ sessionId }) as ResultRecord
const deferred = () => {
  let resolve!: (value: HistoryPage) => void, reject!: (reason: unknown) => void
  const promise = new Promise<HistoryPage>((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}
describe('history query generations', () => {
  it('refreshes completed refinement without hiding the open record or accepting an older page', async () => {
    const first = deferred(),
      more = deferred(),
      fresh = deferred()
    let calls = 0
    const responses = [first, more, fresh]
    const loader = new HistoryLoader(() => responses[calls++].promise)
    const initial = loader.load()
    const raw = { ...record('a'), rawText: 'Original', polishOutcome: 'off' }
    first.resolve({ items: [raw], nextCursor: 'next' })
    await initial
    const stale = loader.load(true)
    const refresh = loader.refresh()
    expect(loader.snapshot().items[0]).toBe(raw)
    fresh.resolve({
      items: [{ ...raw, polishOutcome: 'applied', polishedText: 'Refined' }],
      nextCursor: null,
    })
    await refresh
    more.resolve({ items: [record('obsolete')], nextCursor: 'old' })
    await stale
    expect(loader.snapshot().items).toHaveLength(1)
    expect(loader.snapshot().items[0].polishedText).toBe('Refined')
  })
  it('keeps the newest durable receipt when an older page arrives, without inserting a row', async () => {
    const first = deferred()
    const loader = new HistoryLoader(() => first.promise)
    const pending = loader.load()
    loader.updateDelivery({ sessionId: 'a', deliveryStatus: 'uncertain', deliveryReason: null })
    loader.updateDelivery({ sessionId: 'a', deliveryStatus: 'submitted', deliveryReason: null })
    loader.updateDelivery({
      sessionId: 'absent',
      deliveryStatus: 'copied_only',
      deliveryReason: null,
    })
    expect(loader.snapshot().items).toEqual([])
    first.resolve({
      items: [{ ...record('a'), rawText: 'Keep the original', deliveryStatus: 'ready_for_copy' }],
      nextCursor: 'next',
    })
    await pending
    expect(loader.snapshot().items).toEqual([
      {
        ...record('a'),
        rawText: 'Keep the original',
        deliveryStatus: 'submitted',
        deliveryReason: null,
      },
    ])
    expect(loader.snapshot().nextCursor).toBe('next')
  })
  it('updates loaded records in place while pagination continues, preserving text and cursor', async () => {
    const first = deferred(),
      more = deferred()
    let calls = 0
    const loader = new HistoryLoader(() => (calls++ === 0 ? first : more).promise)
    const initial = loader.load()
    const a = { ...record('a'), rawText: 'Original A' },
      b = record('b')
    first.resolve({ items: [a, b], nextCursor: 'next' })
    await initial
    const pending = loader.load(true)
    loader.updateDelivery({
      sessionId: 'a',
      deliveryStatus: 'copied_only',
      deliveryReason: 'target_changed',
    })
    expect(loader.snapshot().items.map((r) => r.sessionId)).toEqual(['a', 'b'])
    expect(loader.snapshot().items[1]).toBe(b)
    expect(loader.snapshot().items[0]).toMatchObject({
      rawText: a.rawText,
      deliveryStatus: 'copied_only',
      deliveryReason: 'target_changed',
    })
    expect(loader.snapshot()).toMatchObject({ loading: true, nextCursor: 'next' })
    more.resolve({ items: [a, record('c')], nextCursor: null })
    await pending
    expect(loader.snapshot().items.map((r) => r.sessionId)).toEqual(['a', 'b', 'c'])
    expect(loader.snapshot().items[0].deliveryStatus).toBe('copied_only')
  })
  it('does not append an old page or clear a newer loading state after search changes', async () => {
    const first = deferred(),
      more = deferred(),
      fresh = deferred()
    const pending = [first, more, fresh]
    let calls = 0
    const loader = new HistoryLoader(() => pending[calls++].promise)
    const a = loader.load()
    first.resolve({ items: [record('a')], nextCursor: 'cursor' })
    await a
    const b = loader.load(true)
    await loader.load(true)
    expect(calls).toBe(2)
    loader.reset('new')
    const c = loader.load()
    more.resolve({ items: [record('obsolete')], nextCursor: null })
    await b
    expect(loader.snapshot().items).toEqual([])
    expect(loader.snapshot().loading).toBe(true)
    fresh.resolve({ items: [record('fresh')], nextCursor: null })
    await c
    expect(loader.snapshot().items.map((r) => r.sessionId)).toEqual(['fresh'])
  })
  it('keeps loaded pages and their cursor when a new dictation refreshes the first page', async () => {
    const pages = [
      { items: [record('a'), record('b')], nextCursor: 'after-b' },
      { items: [record('c'), record('d')], nextCursor: 'after-d' },
      { items: [record('new'), record('a')], nextCursor: 'after-a' },
    ]
    let calls = 0
    const loader = new HistoryLoader(async () => pages[calls++])
    await loader.load()
    await loader.load(true)
    const c = loader.snapshot().items[2]
    await loader.refresh()
    expect(loader.snapshot().items.map((r) => r.sessionId)).toEqual(['new', 'a', 'b', 'c', 'd'])
    expect(loader.snapshot().items[3]).toBe(c)
    expect(loader.snapshot()).toMatchObject({ nextCursor: 'after-d', loading: false })
  })
  it('drops loaded rows a new dictation pruned by the history limit or age', async () => {
    const now = Date.now(),
      at = (sessionId: string, createdAt: number) => ({ ...record(sessionId), createdAt })
    const pages = [
      { items: [at('a', now - 1000), at('b', now - 2000)], nextCursor: 'after-b' },
      { items: [at('c', now - 3000), at('d', now - 2 * 86_400_000)], nextCursor: 'after-d' },
      { items: [at('new', now), at('a', now - 1000)], nextCursor: 'after-a' },
      { items: [at('newer', now), at('new', now)], nextCursor: 'after-new' },
    ]
    let calls = 0
    const loader = new HistoryLoader(async () => pages[calls++])
    await loader.load()
    await loader.load(true)
    const b = loader.snapshot().items[1]
    await loader.refresh({ limit: 100, days: 1 })
    expect(loader.snapshot().items.map((r) => r.sessionId)).toEqual(['new', 'a', 'b', 'c'])
    expect(loader.snapshot().items[2]).toBe(b)
    expect(loader.snapshot().nextCursor).toBeNull()
    await loader.refresh({ limit: 3, days: 30 })
    expect(loader.snapshot().items.map((r) => r.sessionId)).toEqual(['newer', 'new', 'a'])
    expect(loader.snapshot().nextCursor).toBeNull()
  })
  it('ignores stale errors and supports retry without skipping a failed page', async () => {
    const old = deferred(),
      retry = deferred()
    let calls = 0
    const loader = new HistoryLoader(() => (calls++ === 0 ? old : retry).promise)
    const first = loader.load()
    loader.reset('current')
    const second = loader.load()
    old.reject(new Error('obsolete'))
    await first
    expect(loader.snapshot().error).toBeNull()
    retry.resolve({ items: [record('ok')], nextCursor: 'same' })
    await second
    expect(loader.snapshot().nextCursor).toBe('same')
    loader.invalidate()
  })
})
