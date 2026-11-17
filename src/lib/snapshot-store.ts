import type { Snapshot } from './types'

export interface LiveSession {
  elapsedMs: number
  audioLevel: number
}

// While dictating, the native session republishes about every 200 ms only to
// advance the timer and the input level. Keep those values in this small store
// so only the components that draw them re-render at that rate.
export class LiveSessionStore {
  private value: LiveSession = { elapsedMs: 0, audioLevel: 0 }
  private listeners = new Set<() => void>()
  snapshot = () => this.value
  subscribe = (fn: () => void) => {
    this.listeners.add(fn)
    return () => {
      this.listeners.delete(fn)
    }
  }
  set(elapsedMs: number, audioLevel: number) {
    if (elapsedMs === this.value.elapsedMs && audioLevel === this.value.audioLevel) return
    this.value = { elapsedMs, audioLevel }
    this.listeners.forEach((fn) => fn())
  }
}

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value)
const identity = (value: unknown) => {
  const key = isRecord(value) ? (value.sessionId ?? value.entryId ?? value.id) : undefined
  return typeof key === 'string' ? key : undefined
}

// Reuse every unchanged branch of `previous`, so memoized consumers receive the
// same references (and `previous` itself when nothing changed). Array items with
// an id are matched by id, so a result inserted at the front keeps the others.
export function shareEqual<T>(previous: unknown, next: T): T {
  if (Object.is(previous, next)) return previous as T
  if (Array.isArray(previous) && Array.isArray(next)) {
    const byId = new Map<string, unknown>()
    for (const item of previous) {
      const key = identity(item)
      if (key !== undefined) byId.set(key, item)
    }
    let same = previous.length === next.length
    const merged = next.map((item, i) => {
      const key = identity(item)
      const value = shareEqual(
        key !== undefined && byId.has(key) ? byId.get(key) : previous[i],
        item,
      )
      if (value !== previous[i]) same = false
      return value
    })
    return (same ? previous : merged) as T
  }
  if (isRecord(previous) && isRecord(next)) {
    const keys = Object.keys(next)
    let same = keys.length === Object.keys(previous).length
    const merged: Record<string, unknown> = {}
    for (const key of keys) {
      const value = shareEqual(previous[key], next[key])
      if (value !== previous[key] || !Object.hasOwn(previous, key)) same = false
      merged[key] = value
    }
    return (same ? previous : merged) as T
  }
  return next
}

// Returns `current` unchanged when a newer snapshot differs only in its
// revision, elapsed time or input level; those are read from LiveSessionStore.
export function mergeSnapshot(current: Snapshot | null, next: Snapshot): Snapshot {
  if (!current) return next
  const steady = shareEqual(current, {
    ...next,
    revision: current.revision,
    audioLevel: current.audioLevel,
    session: { ...next.session, elapsedMs: current.session.elapsedMs },
  })
  if (steady === current) return current
  return {
    ...steady,
    revision: next.revision,
    audioLevel: next.audioLevel,
    session:
      steady.session.elapsedMs === next.session.elapsedMs
        ? steady.session
        : { ...steady.session, elapsedMs: next.session.elapsedMs },
  }
}
