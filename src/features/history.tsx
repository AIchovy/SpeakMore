import { Checkbox } from '@/components/ui/checkbox'
import { asAppError, type UiMessage } from '@/lib/errors'
import { memo, useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react'
import { toast } from 'sonner'
import { Link } from 'react-router-dom'
import { isTauri } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { call, useApp, useText } from '@/lib/app-state'
import { type DeliveryUpdate, type HistoryPage, type ResultRecord } from '@/lib/types'
import { HistoryLoader, type HistoryRetention } from '@/lib/history-loader'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from '@/components/ui/dialog'
import { PageTitle, EmptyState, InlineError, Confirm, Choice } from '@/components/common'
import { DownloadIcon, HistoryIcon, SearchIcon, TrashIcon } from '@/components/icons'
import { defaultVariant, deliveryLabel, resultPreview, ResultCopy, ResultDetail } from './results'

type ExportScope = { kind: 'selected'; ids: string[] } | { kind: 'filtered'; query: string }
type ExportDraft = { scope: ExportScope; format: 'txt' | 'csv' }
// Same output as Date#toLocaleString(), created once instead of per row and render.
const timestamp = new Intl.DateTimeFormat(undefined, {
  year: 'numeric',
  month: 'numeric',
  day: 'numeric',
  hour: 'numeric',
  minute: 'numeric',
  second: 'numeric',
})
// Memoized so snapshot updates and checking one row do not rebuild every row.
const HistoryRow = memo(function HistoryRow({
  item,
  checked,
  busy,
  onCheck,
  onOpen,
  onDelete,
}: {
  item: ResultRecord
  checked: boolean
  busy: boolean
  onCheck: (id: string, checked: boolean) => void
  onOpen: (id: string, trigger: HTMLButtonElement) => void
  onDelete: (id: string) => void
}) {
  const t = useText()
  const at = timestamp.format(item.createdAt)
  return (
    <div className="flex items-start gap-3 border-b px-5 py-4 last:border-b-0">
      <Checkbox
        className="mt-1 size-4 shrink-0"
        aria-label={t(`选择 ${at} 的记录`, `Select result from ${at}`)}
        checked={checked}
        disabled={busy}
        onCheckedChange={(value) => onCheck(item.sessionId, value === true)}
      />
      <button
        onClick={(event) => onOpen(item.sessionId, event.currentTarget)}
        className="min-w-0 flex-1 text-left"
        aria-label={`${t('查看结果：', 'View result: ')}${resultPreview(item)}`}
      >
        <p className="line-clamp-2 break-words text-sm leading-6">
          {defaultVariant(item) === 'polished' ? item.polishedText : item.rawText}
        </p>
        <p className="mt-2 text-xs text-muted-foreground">
          {at} · {Math.round(item.durationMs / 1000)} s · {deliveryLabel(item.deliveryStatus, t)}
        </p>
      </button>
      <div className="flex shrink-0 flex-wrap items-start gap-1">
        <ResultCopy record={item} />
        <Button
          size="icon-sm"
          variant="ghost"
          aria-label={t('删除记录', 'Delete result')}
          onClick={() => onDelete(item.sessionId)}
        >
          <TrashIcon aria-hidden />
        </Button>
      </div>
    </div>
  )
})
export function History() {
  const { data } = useApp(),
    t = useText()
  const [loader] = useState(
    () =>
      new HistoryLoader((query, cursor) => call<HistoryPage>('list_history', { query, cursor })),
  )
  const detailTrigger = useRef<HTMLButtonElement | null>(null),
    exportTrigger = useRef<HTMLButtonElement | null>(null),
    searchInput = useRef<HTMLInputElement | null>(null)
  const page = useSyncExternalStore(loader.subscribe, loader.snapshot, loader.snapshot)
  const [query, setQuery] = useState(''),
    [refresh, setRefresh] = useState(0)
  const [historyEventsReady, setHistoryEventsReady] = useState(() => !isTauri())
  // A new dictation prunes rows past the limit or age without an event; refreshes drop them.
  const retention = useRef<HistoryRetention | undefined>(undefined)
  retention.current = data
    ? { limit: data.settings.historyLimit, days: data.settings.historyDays }
    : undefined
  const phase = data?.session.phase,
    latestResult = data?.recentResults[0]?.sessionId
  const previousPhase = useRef(phase),
    previousResult = useRef(latestResult)
  // New dictations arrive when the session returns to idle or a new result appears; refresh
  // rather than reset to keep the selection, the open detail and loaded pages.
  useEffect(() => {
    const finished = phase === 'idle' && !!previousPhase.current && previousPhase.current !== 'idle'
    const added = !!latestResult && latestResult !== previousResult.current
    if (historyEventsReady && (finished || added)) void loader.refresh(retention.current)
    previousPhase.current = phase
    previousResult.current = latestResult
  }, [phase, latestResult, historyEventsReady, loader])
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [checked, setChecked] = useState<Set<string>>(() => new Set())
  const [deleting, setDeleting] = useState<string[] | null>(null)
  const [draft, setDraft] = useState<ExportDraft | null>(null),
    [exporting, setExporting] = useState(false),
    [exportError, setExportError] = useState<UiMessage | null>(null)
  // A new search, history_changed or a retry starts over; new dictations use the refresh above.
  useEffect(() => {
    if (!historyEventsReady) return
    loader.reset(query)
    setChecked(new Set())
    setSelectedId(null)
    const timer = setTimeout(() => void loader.load(), 200)
    return () => {
      clearTimeout(timer)
      loader.invalidate()
    }
  }, [loader, query, refresh, historyEventsReady])
  const toggleChecked = useCallback(
    (id: string, value: boolean) =>
      setChecked((old) => {
        const next = new Set(old)
        if (value) next.add(id)
        else next.delete(id)
        return next
      }),
    [],
  )
  const openDetail = useCallback((id: string, trigger: HTMLButtonElement) => {
    detailTrigger.current = trigger
    setSelectedId(id)
  }, [])
  const askDelete = useCallback((id: string) => setDeleting([id]), [])
  useEffect(() => {
    let disposed = false
    const off: UnlistenFn[] = []
    // Subscribe before the first query so its reply cannot miss a newer receipt.
    if (isTauri()) {
      const subscriptions = [
        listen('history_changed', () => setRefresh((v) => v + 1)),
        listen<DeliveryUpdate>('history_delivery_changed', ({ payload }) =>
          loader.updateDelivery(payload),
        ),
      ]
      void Promise.allSettled(
        subscriptions.map((subscription) =>
          subscription.then((fn) => {
            if (disposed) fn()
            else off.push(fn)
          }),
        ),
      ).then(() => {
        if (!disposed) setHistoryEventsReady(true)
      })
    }
    return () => {
      disposed = true
      off.forEach((fn) => fn())
    }
  }, [loader])
  // Rows a refresh removed (pruned or deleted elsewhere) leave the selection too.
  useEffect(() => {
    setChecked((old) => {
      if (!old.size) return old
      const listed = new Set(page.items.map((r) => r.sessionId))
      const kept = [...old].filter((id) => listed.has(id))
      return kept.length === old.size ? old : new Set(kept)
    })
  }, [page.items])
  const selected = page.items.find((r) => r.sessionId === selectedId) ?? null
  const allChecked = page.items.length > 0 && page.items.every((r) => checked.has(r.sessionId))
  const busy = page.loading || page.query !== query
  const scopeLabel = (scope: ExportScope) =>
    scope.kind === 'selected'
      ? t(`已选 ${scope.ids.length} 条记录`, `${scope.ids.length} selected records`)
      : scope.query
        ? t(
            `搜索“${scope.query}”的全部匹配记录（含未加载记录）`,
            `All matches for “${scope.query}”, including unloaded records`,
          )
        : t('全部历史记录（含未加载记录）', 'All history, including unloaded records')
  if (!data) return null
  return (
    <>
      <PageTitle
        title={t('历史记录', 'History')}
        description={t(
          '回看说过的话，找回值得留下的表达。',
          'Revisit your words and recover the thoughts you want to keep.',
        )}
        action={
          <Button
            ref={exportTrigger}
            variant="outline"
            disabled={!data.desktop || busy || !page.items.length}
            onClick={() => {
              setExportError(null)
              setDraft({
                scope: checked.size
                  ? { kind: 'selected', ids: [...checked] }
                  : { kind: 'filtered', query },
                format: 'txt',
              })
            }}
          >
            <DownloadIcon aria-hidden />
            {t('导出', 'Export')}
          </Button>
        }
      />
      {!data.settings.historyEnabled && (
        <div className="mb-6 flex flex-wrap items-center justify-between gap-3 rounded-lg border bg-card px-5 py-4">
          <div>
            <p className="font-medium">{t('历史记录已关闭', 'History is off')}</p>
            <p className="mt-1 text-xs text-muted-foreground">
              {t(
                '新录音不会保存。最近结果仍可在概览找回，最长保留 15 分钟。',
                'New transcripts aren’t saved. Recent results remain available for 15 minutes.',
              )}
            </p>
          </div>
          <Button asChild variant="outline" size="sm">
            <Link to="/settings/privacy">{t('隐私设置', 'Privacy settings')}</Link>
          </Button>
        </div>
      )}
      <div className="mb-5 flex flex-wrap items-center justify-between gap-3">
        <div className="relative w-full max-w-[360px]">
          <SearchIcon
            className="absolute top-2.5 left-3 size-4 text-muted-foreground"
            aria-hidden
          />
          <Input
            ref={searchInput}
            aria-label={t('搜索历史记录', 'Search history')}
            className="border-border bg-card pl-9"
            placeholder={t('搜索转写内容…', 'Search your transcripts…')}
            value={query}
            onChange={(e) => {
              loader.reset(e.target.value)
              setQuery(e.target.value)
              setChecked(new Set())
              setSelectedId(null)
            }}
          />
        </div>
        {checked.size > 0 && (
          <div className="flex items-center gap-3 text-sm">
            <span>{t(`已选 ${checked.size} 条`, `${checked.size} selected`)}</span>
            <Button
              variant="outline"
              size="sm"
              disabled={busy}
              onClick={() => setDeleting([...checked])}
            >
              <TrashIcon aria-hidden />
              {t('删除所选', 'Delete selected')}
            </Button>
          </div>
        )}
      </div>
      <InlineError message={page.error} />
      {page.error && (
        <Button className="my-3" variant="outline" onClick={() => setRefresh((v) => v + 1)}>
          {t('重新读取', 'Retry')}
        </Button>
      )}
      <div className="rounded-xl border bg-card" aria-busy={busy}>
        {page.items.length > 0 && (
          <label className="flex items-center gap-3 border-b px-5 py-3 text-xs text-muted-foreground">
            <Checkbox
              className="size-4"
              checked={allChecked}
              disabled={busy}
              onCheckedChange={(value) =>
                setChecked(value === true ? new Set(page.items.map((r) => r.sessionId)) : new Set())
              }
            />
            {t(
              `选择已加载的 ${page.items.length} 条记录`,
              `Select ${page.items.length} loaded records`,
            )}
          </label>
        )}
        {page.items.length === 0 ? (
          <EmptyState
            icon={HistoryIcon}
            title={
              busy
                ? t('正在读取…', 'Loading…')
                : query
                  ? t('没有找到匹配记录', 'No matching results')
                  : t('这里会留下你的表达', 'A place for your words')
            }
            description={
              query
                ? t('试试其他关键词，或清除筛选。', 'Try another keyword, or clear your search.')
                : t(
                    '开启历史记录后，完成的转写将保存在这台设备上。你可以随时删除。',
                    'Turn history on to keep completed transcripts on this device. Delete them whenever you like.',
                  )
            }
          />
        ) : (
          page.items.map((item) => (
            <HistoryRow
              key={item.sessionId}
              item={item}
              checked={checked.has(item.sessionId)}
              busy={busy}
              onCheck={toggleChecked}
              onOpen={openDetail}
              onDelete={askDelete}
            />
          ))
        )}
      </div>
      {page.nextCursor && (
        <Button
          variant="outline"
          className="mt-5"
          disabled={busy}
          onClick={() => void loader.load(true)}
        >
          {t('加载更多', 'Load more')}
        </Button>
      )}
      <ResultDetail
        record={selected}
        onClose={() => setSelectedId(null)}
        restoreFocus={() =>
          (detailTrigger.current?.isConnected
            ? detailTrigger.current
            : searchInput.current
          )?.focus()
        }
      />
      <Confirm
        open={deleting !== null}
        onOpenChange={(open) => {
          if (!open) setDeleting(null)
        }}
        title={t(
          `删除这 ${deleting?.length ?? 0} 条记录？`,
          `Delete ${deleting?.length ?? 0} records?`,
        )}
        description={t(
          '将从历史中永久删除所选记录，使用次数和未过期的临时结果保持不变。',
          'Permanently removes selected history records. Usage counts and unexpired temporary results are unchanged.',
        )}
        onConfirm={async () => {
          await call('delete_history', { ids: deleting })
          setSelectedId(null)
          setChecked(new Set())
          setRefresh((v) => v + 1)
        }}
      />
      <Dialog
        open={draft !== null}
        onOpenChange={(open) => {
          if (!open && !exporting) setDraft(null)
        }}
      >
        {draft && (
          <DialogContent
            closeLabel={t('关闭', 'Close')}
            showCloseButton={!exporting}
            onCloseAutoFocus={(event) => {
              event.preventDefault()
              ;(exportTrigger.current?.disabled
                ? searchInput.current
                : exportTrigger.current
              )?.focus()
            }}
            onEscapeKeyDown={(e) => {
              if (exporting) e.preventDefault()
            }}
          >
            <DialogHeader>
              <DialogTitle>{t('导出历史记录', 'Export history')}</DialogTitle>
              <DialogDescription>{scopeLabel(draft.scope)}</DialogDescription>
            </DialogHeader>
            <p className="text-sm text-muted-foreground">
              {t(
                '选择保存位置后，导出该范围内当时存在的记录。不会开启历史保存。',
                'Records in this scope are read after you choose a location. Exporting does not enable history.',
              )}
            </p>
            <Choice
              label={t('文件格式', 'File format')}
              value={draft.format}
              disabled={exporting}
              options={[
                ['txt', 'TXT'],
                ['csv', 'CSV'],
              ]}
              onChange={(format) => setDraft({ ...draft, format: format as 'txt' | 'csv' })}
            />
            <p className="text-xs leading-6 text-muted-foreground">
              {draft.format === 'txt'
                ? t(
                    '保留原文和整理后的文字，适合阅读与备份。',
                    'Preserves original and refined text for reading and backup.',
                  )
                : t(
                    '包含时间、模型和交付状态。可能被当作公式的单元格添加 text: 前缀；无损文字请选择 TXT。',
                    'Includes time, model and delivery status. Formula-like cells receive a text: prefix; use TXT for unchanged text.',
                  )}
            </p>
            <InlineError message={exportError} />
            <DialogFooter>
              <Button variant="outline" disabled={exporting} onClick={() => setDraft(null)}>
                {t('取消', 'Cancel')}
              </Button>
              <Button
                disabled={exporting}
                onClick={async () => {
                  setExporting(true)
                  setExportError(null)
                  try {
                    const count = await call<number | null>('export_history', draft)
                    if (count !== null) {
                      setDraft(null)
                      toast.success(t(`已导出 ${count} 条记录`, `Exported ${count} records`))
                    }
                  } catch (e) {
                    setExportError(asAppError(e))
                  } finally {
                    setExporting(false)
                  }
                }}
              >
                {exporting
                  ? t('等待保存…', 'Waiting to save…')
                  : t('选择保存位置', 'Choose location')}
              </Button>
            </DialogFooter>
          </DialogContent>
        )}
      </Dialog>
    </>
  )
}
