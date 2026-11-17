import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  applyCaptionPatch,
  captionSync,
  upsertCaptionLine,
  type CaptionLine,
  type CaptionLinePatch,
  type CaptionView,
} from './captions'

const line = (id: string, startMs: number, original = id): CaptionLine => ({
  id,
  startMs,
  endMs: startMs + 100,
  original,
  translated: '',
  finalText: false,
})

describe('caption line patches', () => {
  it('updates a line in place and keeps unchanged rows identical', () => {
    const a = line('a', 0),
      b = line('b', 100)
    const next = upsertCaptionLine([a, b], { ...b, original: 'b2', finalText: true })
    expect(next.map((l) => l.original)).toEqual(['a', 'b2'])
    expect(next[0]).toBe(a)
  })

  it('inserts by start time and id and bounds the list like the native side', () => {
    const lines = Array.from({ length: 900 }, (_, i) => line(`id-${String(i).padStart(4, '0')}`, i))
    const next = upsertCaptionLine(lines, line('id-late', 5000))
    expect(next).toHaveLength(900)
    expect(next[0].id).toBe('id-0001')
    expect(next[899].id).toBe('id-late')
    const between = upsertCaptionLine([line('b', 0), line('d', 200)], line('c', 100))
    expect(between.map((l) => l.id)).toEqual(['b', 'c', 'd'])
    const tie = upsertCaptionLine([line('b', 100)], line('a', 100))
    expect(tie.map((l) => l.id)).toEqual(['a', 'b'])
  })

  it('applies translation status with the revision', () => {
    const view = { lines: [line('a', 0)], revision: 4 } as CaptionView
    const next = applyCaptionPatch(view, {
      revision: 5,
      line: null,
      translationStatus: 'retrying',
      translationError: 'busy',
    })
    expect(next.lines).toBe(view.lines)
    expect(next.revision).toBe(5)
    expect(next.translationStatus).toBe('retrying')
  })
})

describe('caption view sync', () => {
  afterEach(() => {
    vi.useRealTimers()
  })
  const view = (revision: number, lines: CaptionLine[] = []) =>
    ({ revision, lines, translationStatus: 'idle', translationError: null }) as CaptionView
  const patch = (revision: number, l: CaptionLine | null = null): CaptionLinePatch => ({
    revision,
    line: l,
    translationStatus: 'idle',
    translationError: null,
  })
  const setup = () => {
    const reads: { resolve: (v: CaptionView) => void; reject: (e: unknown) => void }[] = []
    const views: CaptionView[] = []
    const sync = captionSync(
      () =>
        new Promise<CaptionView>((resolve, reject) => {
          reads.push({ resolve, reject })
        }),
      (v) => views.push(v),
      () => {},
    )
    return { sync, reads, views, last: () => views[views.length - 1] }
  }
  it('keeps line patches that arrive before the first read and applies them after it', async () => {
    vi.useFakeTimers()
    const { sync, reads, last } = setup()
    sync.reload()
    sync.patch(patch(5, { ...line('a', 0), finalText: true }))
    await vi.advanceTimersByTimeAsync(50)
    expect(reads).toHaveLength(1)
    reads[0].resolve(view(4, [line('a', 0)]))
    await vi.advanceTimersByTimeAsync(0)
    expect(last().revision).toBe(5)
    expect(last().lines[0].finalText).toBe(true)
  })
  it('holds patches after a gap until the read lands, then applies the newer ones', async () => {
    vi.useFakeTimers()
    const { sync, reads, last } = setup()
    sync.reload()
    reads[0].resolve(view(1))
    await vi.advanceTimersByTimeAsync(0)
    sync.patch(patch(4, line('b', 100)))
    await vi.advanceTimersByTimeAsync(50)
    expect(reads).toHaveLength(2)
    // The final line arrives while the full read is still in flight.
    sync.patch(patch(5, { ...line('b', 100), finalText: true, translated: 'B' }))
    await vi.advanceTimersByTimeAsync(50)
    expect(reads).toHaveLength(2)
    reads[1].resolve(view(3, [line('a', 0)]))
    await vi.advanceTimersByTimeAsync(0)
    expect(reads).toHaveLength(2)
    expect(last().revision).toBe(5)
    expect(last().lines.map((l) => [l.id, l.finalText, l.translated])).toEqual([
      ['a', false, ''],
      ['b', true, 'B'],
    ])
  })
  it('skips patches the read already covers and coalesces reads into one trailing read', async () => {
    vi.useFakeTimers()
    const { sync, reads, last } = setup()
    sync.reload()
    sync.reload()
    sync.reload()
    sync.patch(patch(2, line('a', 0)))
    await vi.advanceTimersByTimeAsync(50)
    expect(reads).toHaveLength(1)
    reads[0].resolve(view(1))
    await vi.advanceTimersByTimeAsync(0)
    expect(reads).toHaveLength(2)
    reads[1].resolve(view(6, [line('a', 0, 'covered')]))
    await vi.advanceTimersByTimeAsync(0)
    expect(reads).toHaveLength(2)
    expect(last().revision).toBe(6)
    expect(last().lines[0].original).toBe('covered')
  })
})
