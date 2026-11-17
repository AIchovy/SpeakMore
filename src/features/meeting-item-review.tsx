import { useState } from 'react'
import { toast } from 'sonner'
import { useText } from '@/lib/app-state'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { MeetingAudio } from './meeting-audio'
import { meetingCall, meetingError, timecode, type Meeting } from './meetings-api'
import { meetingMessage } from './meeting-messages'

export function reviewSources(meeting: Meeting, index: number) {
  const ids = meeting.items[index]?.evidence || []
  return meeting.segments.filter((segment) => ids.includes(segment.id))
}

export function MeetingItemReview({
  meeting,
  index,
  currentRevision,
  disabled,
  onClose,
  onEdit,
  refresh,
}: {
  meeting: Meeting
  index: number
  currentRevision: number | undefined
  disabled: boolean
  onClose: () => void
  onEdit: () => void
  refresh: () => Promise<unknown>
}) {
  const t = useText()
  const [pending, setPending] = useState(false)
  const [error, setError] = useState('')
  const item = meeting.items[index]
  const sources = reviewSources(meeting, index)
  const changed = currentRevision !== meeting.revision
  const blocked = pending || disabled || changed
  const verify = async (verified: boolean) => {
    if (blocked || (verified && !sources.length)) return
    setPending(true)
    setError('')
    try {
      await meetingCall('meeting_item_verify', {
        id: meeting.id,
        revision: meeting.revision,
        index,
        verified,
      })
      await refresh()
      toast.success(
        verified
          ? t('已确认这条内容与原文一致', 'Confirmed that this item matches the transcript')
          : t('已撤销核对，可重新检查原文', 'Review withdrawn; you can check the transcript again'),
      )
      onClose()
    } catch (e) {
      setError(meetingError(e))
    } finally {
      setPending(false)
    }
  }
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !pending) onClose()
      }}
    >
      <DialogContent
        closeLabel={t('关闭', 'Close')}
        showCloseButton={!pending}
        className="flex flex-col overflow-hidden sm:max-w-2xl"
      >
        <DialogHeader>
          <DialogTitle>{t('核对这条内容', 'Check this item against the transcript')}</DialogTitle>
          <DialogDescription>
            {t(
              '对照下方原文，检查表述、负责人、日期和金额是否一致。确认后会取消这条内容的“需核对”提示；待办是否完成仍由勾选框控制。',
              'Compare the wording, owner, dates and amounts with the excerpts below. Confirming clears this item’s review warning. The task completion checkbox remains separate.',
            )}
          </DialogDescription>
        </DialogHeader>
        <div className="min-h-0 space-y-5 overflow-y-auto py-2">
          <section className="rounded-lg bg-muted/50 p-4">
            <h3 className="mb-2 text-xs font-medium text-muted-foreground">
              {t('需要核对的内容', 'Item to check')}
            </h3>
            <p className="whitespace-pre-wrap text-sm leading-7">{item.text}</p>
            {item.kind === 'todo' && (
              <p className="mt-2 text-xs text-muted-foreground">
                {t('负责人：', 'Owner: ')}
                {item.owner || t('尚未明确', 'Unspecified')} · {t('截止日期：', 'Due: ')}
                {item.due || t('尚未明确', 'Unspecified')}
              </p>
            )}
          </section>
          <section className="space-y-4">
            <h3 className="text-sm font-medium">{t('引用的原文', 'Cited transcript excerpts')}</h3>
            {sources.map((source) => (
              <div key={source.id} className="space-y-3 rounded-lg border p-4">
                <p className="text-xs text-muted-foreground">
                  {timecode(source.startMs)} – {timecode(source.endMs)} ·{' '}
                  {source.speaker || t('说话人未识别', 'Unknown speaker')}
                </p>
                <p className="whitespace-pre-wrap text-sm leading-7">{source.text}</p>
                <MeetingAudio
                  meetingId={meeting.id}
                  startMs={source.startMs}
                  deleted={meeting.audioDeleted}
                />
              </div>
            ))}
            {!sources.length && (
              <p role="alert" className="text-sm leading-6 text-destructive">
                {t(
                  '找不到这条内容引用的原文，暂时无法确认。请先查看逐字稿或重新整理纪要。',
                  'The cited excerpts are missing. Check the transcript or regenerate the notes before confirming.',
                )}
              </p>
            )}
          </section>
          {changed && (
            <p role="alert" className="text-sm leading-6 text-destructive">
              {t(
                '会议内容已更新，请关闭并重新打开核对，避免确认旧内容。',
                'This meeting changed. Close and reopen this review to check the latest content.',
              )}
            </p>
          )}
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {meetingMessage(error, t)}
            </p>
          )}
        </div>
        <DialogFooter className="shrink-0 flex-wrap border-t pt-4">
          <Button variant="ghost" disabled={pending} onClick={onClose}>
            {t('稍后再看', 'Review later')}
          </Button>
          <Button variant="outline" disabled={blocked} onClick={onEdit}>
            {t('修改这条内容', 'Edit this item')}
          </Button>
          {item.reviewed ? (
            <Button disabled={blocked} onClick={() => void verify(false)}>
              {pending ? t('正在撤销…', 'Withdrawing…') : t('撤销这次核对', 'Withdraw this review')}
            </Button>
          ) : (
            <Button disabled={blocked || !sources.length} onClick={() => void verify(true)}>
              {pending
                ? t('正在保存…', 'Saving…')
                : t('内容与原文一致', 'This matches the transcript')}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
