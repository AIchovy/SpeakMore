import {
  asAppError,
  type AppError,
  type DeliveryUpdate,
  type HistoryPage,
  type ResultRecord,
} from './types'
type View = HistoryPage & { query: string; loading: boolean; error: AppError | null }
/** The history limit and age in days, as in settings. */
export type HistoryRetention = { limit: number; days: number }
export class HistoryLoader {
  private epoch = 0
  // Retain only metadata, bounded like history. An in-flight page may predate an event.
  private delivery = new Map<string, DeliveryUpdate>()
  private listeners = new Set<() => void>()
  private view: View = { items: [], nextCursor: null, query: '', loading: false, error: null }
  constructor(private fetch: (query: string, cursor: string | null) => Promise<HistoryPage>) {}
  snapshot = () => this.view
  subscribe = (fn: () => void) => {
    this.listeners.add(fn)
    return () => {
      this.listeners.delete(fn)
    }
  }
  private publish(patch: Partial<View>) {
    this.view = { ...this.view, ...patch }
    this.listeners.forEach((fn) => fn())
  }
  reset(query: string) {
    this.epoch++
    this.publish({ query, items: [], nextCursor: null, loading: false, error: null })
  }
  invalidate() {
    this.epoch++
  }
  // Keep an open detail visible while replacing a page that predates refinement.
  // Rows loaded beyond the first page stay listed after the refreshed page, except those a new
  // dictation pruned by count or age (which sends no event).
  async refresh(retention?: HistoryRetention) {
    this.epoch++
    this.publish({ loading: false })
    await this.load(false, true, retention)
  }
  private withDelivery(record: ResultRecord): ResultRecord {
    const update = this.delivery.get(record.sessionId)
    return !update ||
      (record.deliveryStatus === update.deliveryStatus &&
        record.deliveryReason === update.deliveryReason)
      ? record
      : { ...record, ...update }
  }
  updateDelivery({ sessionId, deliveryStatus, deliveryReason }: DeliveryUpdate) {
    this.delivery.delete(sessionId)
    this.delivery.set(sessionId, { sessionId, deliveryStatus, deliveryReason })
    if (this.delivery.size > 5000) this.delivery.delete(this.delivery.keys().next().value!)
    const items = this.view.items.map((record) => this.withDelivery(record))
    if (items.some((record, i) => record !== this.view.items[i])) this.publish({ items })
  }
  async load(more = false, keepLoaded = false, retention?: HistoryRetention) {
    if (this.view.loading || (more && !this.view.nextCursor)) return
    const epoch = this.epoch,
      query = this.view.query,
      cursor = more ? this.view.nextCursor : null
    this.publish({ loading: true, error: null })
    try {
      const page = await this.fetch(query, cursor)
      if (epoch !== this.epoch) return
      if (keepLoaded) {
        const fresh = page.items.map((r) => this.withDelivery(r))
        const ids = new Set(fresh.map((r) => r.sessionId))
        const older = this.view.items.filter((r) => !ids.has(r.sessionId))
        // Older rows continue after the refreshed first page; their cursor still applies.
        let items = [...fresh, ...older],
          nextCursor = older.length ? this.view.nextCursor : page.nextCursor
        if (retention) {
          // Rows are newest first, so a row past the limit (even within a search) or older than
          // the cutoff has been pruned, and so has everything after it.
          const cutoff = Date.now() - retention.days * 86_400_000
          const kept = items.filter((r, i) => i < retention.limit && r.createdAt >= cutoff)
          if (kept.length < items.length) {
            items = kept
            nextCursor = null
          }
        }
        this.publish({ items, nextCursor })
        return
      }
      const existing = more ? this.view.items : []
      const ids = new Set(existing.map((r) => r.sessionId))
      this.publish({
        items: [
          ...existing,
          ...page.items.filter((r) => !ids.has(r.sessionId)).map((r) => this.withDelivery(r)),
        ],
        nextCursor: page.nextCursor,
      })
    } catch (e) {
      if (epoch === this.epoch) this.publish({ error: asAppError(e) })
    } finally {
      if (epoch === this.epoch) this.publish({ loading: false })
    }
  }
}
