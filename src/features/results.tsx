import { asAppError, type UiMessage } from '@/lib/errors'
import { useEffect, useRef, useState } from 'react'
import { call, useText } from '@/lib/app-state'
import { type ResultRecord } from '@/lib/types'
import { Button } from '@/components/ui/button'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { InlineError, EmptyState } from '@/components/common'
import { CopyIcon, HistoryIcon, ArrowRightIcon } from '@/components/icons'

export function defaultVariant(record: ResultRecord): 'raw' | 'polished' {
  return record.polishOutcome === 'applied' && record.polishedText ? 'polished' : 'raw'
}
export function refinementReason(
  code: string | null | undefined,
  t: (zh: string, en: string) => string,
): string | null {
  const reasons: Record<string, [string, string]> = {
    pro_required: [
      '此历史结果曾被旧版本的许可限制跳过整理；当前版本无需许可。',
      'An older version skipped refinement for this result due to licensing. This version needs no license.',
    ],
    provider_unavailable: [
      'AI 服务未就绪，请检查配置并测试连接。',
      'The AI service is not ready. Check its settings and test the connection.',
    ],
    policy_revoked: [
      '整理已关闭，或当前隐私设置不允许此服务。',
      'Refinement is off, or your privacy settings no longer allow this service.',
    ],
    cancelled: ['本次整理已取消。', 'Refinement was cancelled.'],
    deadline: ['AI 服务未在 2.5 秒内完成。', 'The AI service did not finish within 2.5 seconds.'],
    text_too_long: [
      '文字超过 4,000 字符，已跳过整理。',
      'Refinement was skipped because the text exceeds 4,000 characters.',
    ],
    needs_review: [
      '识别结果需要复核，已跳过整理。',
      'Refinement was skipped because the transcript needs review.',
    ],
    invalid_prompt: [
      '自定义整理要求无效，请检查后重试。',
      'Check your custom refinement instructions before trying again.',
    ],
    network_unavailable: [
      'AI 服务连接失败或拒绝请求。',
      'The AI service could not be reached or rejected the request.',
    ],
    response_rejected: [
      '返回内容未通过完整性检查，或改动了数字、链接等受保护内容。',
      'The response was incomplete, invalid, or changed protected content such as numbers or links.',
    ],
  }
  const words = code ? reasons[code] : undefined
  return words ? t(...words) : null
}
export function resultPreview(record: ResultRecord) {
  const text = defaultVariant(record) === 'polished' ? record.polishedText! : record.rawText
  const characters = Array.from(text.replace(/\s+/g, ' ').trim())
  return characters.slice(0, 80).join('') + (characters.length > 80 ? '…' : '')
}
export function refinementStatus(record: ResultRecord, t: (zh: string, en: string) => string) {
  if (defaultVariant(record) === 'polished') {
    return record.rawText === record.polishedText
      ? t('AI 已整理，原文无需调整', 'AI refinement completed; no changes needed')
      : t('AI 已整理', 'AI refinement completed')
  }
  if (['fallback', 'skipped'].includes(record.polishOutcome)) {
    const reason = refinementReason(record.polishReason, t)
    return (
      t('整理未完成，已保留原文', 'Refinement did not finish. The original is preserved.') +
      (reason ? ` · ${reason}` : '')
    )
  }
  return t('本次未启用 AI 整理', 'AI refinement was off for this dictation')
}
export function deliveryLabel(status: string, t: (zh: string, en: string) => string) {
  const labels: Record<string, [string, string]> = {
    ready_for_copy: ['待复制', 'Ready to copy'],
    copied_only: ['已复制，待粘贴', 'Copied; ready to paste'],
    submitted: ['已提交粘贴', 'Paste submitted'],
    verified: ['已确认输入', 'Input confirmed'],
    uncertain: ['请检查目标应用', 'Check the target app'],
    cancelled: ['已取消', 'Cancelled'],
    failed: ['交付失败', 'Delivery failed'],
  }
  return t(...(labels[status] ?? ['交付状态未知', 'Delivery status unknown']))
}
export function manualReason(reason: string | null, t: (zh: string, en: string) => string) {
  const labels: Record<string, [string, string]> = {
    main_window: [
      '这次听写从应用内开始，请复制后粘贴。',
      'Dictation started in SpeakMore. Copy the result and paste it where needed.',
    ],
    user_requested: [
      '你已选择手动输入，请复制后粘贴。',
      'You chose manual delivery. Copy and paste the result.',
    ],
    accessibility_required: [
      '辅助功能未授权，文字已保留供手动粘贴。',
      'Accessibility access is unavailable. Your text is ready for manual copying.',
    ],
    unsupported_target: [
      '此输入框尚不支持自动输入，请手动粘贴。',
      'Automatic insertion is not supported in this field. Paste manually.',
    ],
    sensitive_target: [
      '目标是受保护的输入框，请自行选择是否使用这些文字。',
      'The target is a protected field. Decide where to use this text.',
    ],
    read_only_target: [
      '目标输入框无法编辑，请选择其他位置粘贴。',
      'The target field is not editable. Paste into another field.',
    ],
    target_changed: [
      '焦点、选区或文字已变化，请在正确位置手动粘贴。',
      'Focus, selection, or text changed. Paste manually in the correct place.',
    ],
    target_unavailable: [
      '无法确认原输入框，文字已保留供手动粘贴。',
      'The original input field could not be verified. Copy and paste manually.',
    ],
    probe_timeout: [
      '检查输入框超时，文字已保留供手动粘贴。',
      'Checking the input field timed out. Copy and paste manually.',
    ],
    needs_review: ['请先检查识别结果，再复制使用。', 'Review the transcription before copying it.'],
    text_too_long: ['文本较长，请复制后粘贴。', 'This text is long. Copy and paste it manually.'],
    compatibility_unverified: [
      '自动输入尚未完成兼容性验证，请手动粘贴。',
      'Automatic insertion compatibility has not been verified. Paste manually.',
    ],
  }
  return reason && labels[reason] ? t(...labels[reason]) : null
}
export function ResultCopy({
  record,
  variant = defaultVariant(record),
}: {
  record: ResultRecord
  variant?: 'raw' | 'polished'
}) {
  const t = useText()
  const [pending, setPending] = useState(false),
    [copied, setCopied] = useState(false),
    [error, setError] = useState<UiMessage | null>(null)
  const live = useRef(true)
  useEffect(() => {
    live.current = true
    return () => {
      live.current = false
    }
  }, [])
  useEffect(() => {
    if (!copied) return
    const timer = setTimeout(() => setCopied(false), 2000)
    return () => clearTimeout(timer)
  }, [copied])
  return (
    <div className="max-w-[220px] space-y-2">
      <Button
        size="sm"
        variant="outline"
        disabled={pending}
        onClick={async () => {
          setPending(true)
          setError(null)
          setCopied(false)
          try {
            await call('copy_result', { resultId: record.sessionId, variant })
            if (live.current) setCopied(true)
          } catch (e) {
            if (live.current) setError(asAppError(e))
          } finally {
            if (live.current) setPending(false)
          }
        }}
      >
        <CopyIcon aria-hidden />
        {pending
          ? t('正在复制…', 'Copying…')
          : copied
            ? t('已复制', 'Copied')
            : error
              ? t('重试复制', 'Retry copy')
              : t('复制', 'Copy')}
      </Button>
      {copied && (
        <span className="sr-only" role="status">
          {t('已复制到剪贴板', 'Copied to clipboard')}
        </span>
      )}
      <InlineError message={error} />
    </div>
  )
}
export function ResultBody({ record }: { record: ResultRecord }) {
  const t = useText()
  const [variant, setVariant] = useState(defaultVariant(record))
  const reason = manualReason(record.deliveryReason, t)
  const hasRefinedText = !!record.polishedText
  const textClassName =
    'm-0 min-h-0 flex-1 overflow-y-auto whitespace-pre-wrap break-words pr-2 text-sm leading-7 select-text focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring'
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-4">
      <div className="space-y-2 text-sm text-muted-foreground">
        <p>{deliveryLabel(record.deliveryStatus, t)}</p>
        {reason && <p>{reason}</p>}
        {record.needsReview && record.deliveryReason !== 'needs_review' && (
          <p>{t('请检查后使用', 'Review before use')}</p>
        )}
        <p>{refinementStatus(record, t)}</p>
      </div>
      {hasRefinedText ? (
        <Tabs
          value={variant}
          onValueChange={(v) => setVariant(v as 'raw' | 'polished')}
          className="flex min-h-0 flex-1 flex-col gap-4"
        >
          <TabsList variant="line" className="shrink-0" aria-label={t('文字版本', 'Text version')}>
            <TabsTrigger value="raw">{t('原文', 'Original')}</TabsTrigger>
            <TabsTrigger value="polished">{t('整理后', 'Refined')}</TabsTrigger>
          </TabsList>
          <TabsContent value="raw" className={textClassName} tabIndex={0}>
            {record.rawText}
          </TabsContent>
          <TabsContent value="polished" className={textClassName} tabIndex={0}>
            {record.polishedText}
          </TabsContent>
          <div className="shrink-0 border-t pt-4">
            <ResultCopy key={`${record.sessionId}:${variant}`} record={record} variant={variant} />
          </div>
        </Tabs>
      ) : (
        <>
          <h3 className="shrink-0 text-sm font-medium">{t('原文', 'Original')}</h3>
          <div
            className={textClassName}
            role="region"
            aria-label={t('原文', 'Original')}
            tabIndex={0}
          >
            {record.rawText}
          </div>
          <div className="shrink-0 border-t pt-4">
            <ResultCopy record={record} variant="raw" />
          </div>
        </>
      )}
    </div>
  )
}
export function ResultDetail({
  record,
  onClose,
  restoreFocus,
}: {
  record: ResultRecord | null
  onClose: () => void
  restoreFocus: () => void
}) {
  const t = useText()
  return (
    <Sheet
      open={!!record}
      onOpenChange={(open) => {
        if (!open) onClose()
      }}
    >
      {record && (
        <SheetContent
          closeLabel={t('关闭', 'Close')}
          className="w-full gap-0 sm:max-w-[600px]"
          onCloseAutoFocus={(event) => {
            event.preventDefault()
            restoreFocus()
          }}
        >
          <SheetHeader className="p-6 pr-12">
            <SheetTitle>{t('转写详情', 'Transcript')}</SheetTitle>
            <SheetDescription>
              {new Date(record.createdAt).toLocaleString()} · {Math.round(record.durationMs / 1000)}{' '}
              s
            </SheetDescription>
          </SheetHeader>
          <div className="flex min-h-0 flex-1 flex-col px-6 pb-6">
            <ResultBody key={record.sessionId} record={record} />
          </div>
        </SheetContent>
      )}
    </Sheet>
  )
}
export function RecentResults({
  records,
  open,
  selectedId,
  onSelect,
  onClose,
  restoreFocus,
}: {
  records: ResultRecord[]
  open: boolean
  selectedId: string | null
  onSelect: (id: string | null) => void
  onClose: () => void
  restoreFocus: () => void
}) {
  const t = useText()
  // Only retain the ID. An expired or cleared result must disappear even while
  // its detail is open; this drawer never extends the native 15-minute lifetime.
  const selected = records.find((r) => r.sessionId === selectedId)
  return (
    <Sheet
      open={open}
      onOpenChange={(value) => {
        if (!value) onClose()
      }}
    >
      {open && (
        <SheetContent
          closeLabel={t('关闭', 'Close')}
          className="w-full gap-0 sm:max-w-[600px]"
          onCloseAutoFocus={(event) => {
            event.preventDefault()
            restoreFocus()
          }}
        >
          <SheetHeader className="p-6 pr-12">
            <SheetTitle>{t('最近结果', 'Recent results')}</SheetTitle>
            <SheetDescription>
              {t(
                '最多 10 条，保留 15 分钟。退出应用后清除。',
                'Up to 10 results, available for 15 minutes. Cleared when you quit.',
              )}
            </SheetDescription>
          </SheetHeader>
          <div className="flex min-h-0 flex-1 flex-col gap-4 px-6 pb-6">
            {selectedId ? (
              <>
                <Button variant="ghost" className="w-fit shrink-0" onClick={() => onSelect(null)}>
                  {t('返回最近结果', 'Back to recent results')}
                </Button>
                {selected ? (
                  <ResultBody key={selected.sessionId} record={selected} />
                ) : (
                  <EmptyState
                    icon={HistoryIcon}
                    title={t('结果已过期或已清除', 'Result expired or cleared')}
                    description={t(
                      '临时文字不再保留。若此前开启了历史记录，可前往历史页查找。',
                      'Temporary text is no longer retained. Check History if saving was enabled.',
                    )}
                  />
                )}
              </>
            ) : (
              <div className="min-h-0 overflow-y-auto">
                {records.length === 0 ? (
                  <EmptyState
                    icon={HistoryIcon}
                    title={t('暂无最近结果', 'No recent results')}
                    description={t(
                      '完成听写后，可在这里查看完整文字。',
                      'Completed dictation will appear here.',
                    )}
                  />
                ) : (
                  records.map((record) => (
                    <div key={record.sessionId} className="space-y-3 border-b py-4 first:pt-0">
                      <button
                        className="group block w-full text-left"
                        aria-label={`${t('查看结果：', 'View result: ')}${resultPreview(record)}`}
                        onClick={() => onSelect(record.sessionId)}
                      >
                        <span className="mb-2 flex items-center justify-between text-xs text-muted-foreground">
                          {new Date(record.createdAt).toLocaleString()}
                          <ArrowRightIcon aria-hidden />
                        </span>
                        <span className="line-clamp-2 break-words text-sm leading-6">
                          {defaultVariant(record) === 'polished'
                            ? record.polishedText
                            : record.rawText}
                        </span>
                      </button>
                      <div className="flex items-center justify-between gap-3">
                        <span className="text-xs text-muted-foreground">
                          {deliveryLabel(record.deliveryStatus, t)}
                        </span>
                        <ResultCopy record={record} />
                      </div>
                    </div>
                  ))
                )}
              </div>
            )}
          </div>
        </SheetContent>
      )}
    </Sheet>
  )
}
