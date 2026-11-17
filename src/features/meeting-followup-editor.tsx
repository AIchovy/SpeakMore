import { useEffect, useState } from 'react'
import { useText } from '@/lib/app-state'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Checkbox } from '@/components/ui/checkbox'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from '@/components/ui/dialog'
import { MeetingAudio } from './meeting-audio'
import { meetingCall, meetingError, itemStatus, timecode, type Meeting } from './meetings-api'
import { meetingMessage } from './meeting-messages'
import { taskDate, type FollowupTask } from './meeting-followup-model'

export function FollowupEditor({
  task,
  onClose,
  onSaved,
  openSource,
}: {
  task: FollowupTask
  onClose: () => void
  onSaved: () => Promise<void>
  openSource: () => void
}) {
  const t = useText()
  const [meeting, setMeeting] = useState<Meeting | null>(null)
  const [error, setError] = useState('')
  const [pending, setPending] = useState(false)
  const [edit, setEdit] = useState({
    text: task.item.text,
    owner: task.item.owner || '',
    due: task.item.due || '',
    recipient: task.item.recipient || '',
    confirmed: false,
  })
  useEffect(() => {
    let live = true
    void meetingCall<Meeting>('meeting_get', { id: task.meetingId })
      .then((value) => {
        if (!live) return
        if (value.revision !== task.revision) {
          setError(
            t(
              '来源会议已更新，请关闭并重新打开这条待办。你的输入会保留在当前窗口。',
              'The source meeting changed. Close and reopen this task to load its latest version. Your draft stays in this window.',
            ),
          )
        } else setMeeting(value)
      })
      .catch((failure) => {
        if (live) setError(meetingError(failure))
      })
    return () => {
      live = false
    }
  }, [task.meetingId, task.revision, t])
  const sources = meeting?.segments.filter((s) => task.item.evidence.includes(s.id)) || []
  const canConfirm = sources.length > 0 && itemStatus(task.item) === 'commitment'
  const field = (key: 'text' | 'owner' | 'due' | 'recipient', value: string) =>
    setEdit((previous) => ({ ...previous, [key]: value, confirmed: false }))
  const save = async () => {
    if (pending || !meeting) return
    setPending(true)
    setError('')
    try {
      await meetingCall('meeting_task_update', {
        id: task.meetingId,
        revision: meeting.revision,
        index: task.index,
        edit: {
          ...edit,
          owner: edit.owner.trim() || null,
          due: edit.due || null,
          recipient: edit.recipient.trim() || null,
        },
      })
      await onSaved()
      onClose()
    } catch (failure) {
      setError(meetingError(failure))
    } finally {
      setPending(false)
    }
  }
  return (
    <Dialog
      open
      onOpenChange={(value) => {
        if (!value && !pending) onClose()
      }}
    >
      <DialogContent
        closeLabel={t('关闭', 'Close')}
        showCloseButton={!pending}
        className="flex flex-col overflow-hidden sm:max-w-2xl"
      >
        <DialogHeader>
          <DialogTitle>{t('补充与核对待办', 'Edit and check task')}</DialogTitle>
          <DialogDescription>
            {t(
              '对照原文补充负责人、交付对象和具体日期。保存会同步到来源会议；只有明确承诺才能归入个人跟进。',
              'Check the source before setting an owner, recipient and date. Changes sync to the meeting. Personal follow-ups require an explicit commitment.',
            )}
          </DialogDescription>
        </DialogHeader>
        <div className="min-h-0 flex-1 space-y-5 overflow-y-auto py-2">
          <label className="grid gap-3 text-sm">
            <span>{t('要做什么', 'Task')}</span>
            <Textarea
              maxLength={2000}
              value={edit.text}
              disabled={pending}
              onChange={(e) => field('text', e.target.value)}
            />
          </label>
          <div className="grid gap-5 sm:grid-cols-2">
            <label className="grid gap-3 text-sm">
              <span>{t('负责人', 'Owner')}</span>
              <Input
                maxLength={120}
                value={edit.owner}
                disabled={pending}
                placeholder={t('原文里是谁负责？', 'Who is responsible in the source?')}
                onChange={(e) => field('owner', e.target.value)}
              />
            </label>
            <label className="grid gap-3 text-sm">
              <span>{t('截止日期', 'Due date')}</span>
              <Input
                type="date"
                value={taskDate(edit.due) ? edit.due : ''}
                disabled={pending}
                onChange={(e) => field('due', e.target.value)}
              />
            </label>
          </div>
          {edit.due && (
            <Button variant="ghost" size="sm" disabled={pending} onClick={() => field('due', '')}>
              {t('清除日期，留待确认', 'Clear date; confirm it later')}
            </Button>
          )}
          {edit.due && !taskDate(edit.due) && (
            <p className="text-xs leading-6 text-muted-foreground">
              {t(
                `原记录是“${edit.due}”，请选择具体日期后替换；未选择时会保留原描述。`,
                `The original date is “${edit.due}”. Pick a calendar date to replace it; otherwise the description is kept.`,
              )}
            </p>
          )}
          <label className="grid gap-3 text-sm">
            <span>{t('交付给谁（可选）', 'Recipient (optional)')}</span>
            <Input
              maxLength={120}
              value={edit.recipient}
              disabled={pending}
              placeholder={t('只填写原文明示的接收人', 'Only name an explicit recipient')}
              onChange={(e) => field('recipient', e.target.value)}
            />
          </label>
          <section className="space-y-3 border-t pt-4">
            <h3 className="text-sm font-medium">
              {t('来源会议与原文', 'Source meeting and excerpts')}
            </h3>
            <p className="text-xs text-muted-foreground">
              {task.title} · {new Date(task.createdAt).toLocaleDateString(t('zh-CN', 'en'))}
            </p>
            {!meeting && !error && (
              <p role="status" className="text-sm">
                {t('正在加载原文…', 'Loading source…')}
              </p>
            )}
            {sources.map((s) => (
              <div key={s.id} className="space-y-3 rounded-lg bg-muted/40 p-4">
                <p className="text-xs text-muted-foreground">
                  {timecode(s.startMs)} · {s.speaker || t('说话人未识别', 'Unknown speaker')}
                </p>
                <p className="whitespace-pre-wrap text-sm leading-7">{s.text}</p>
                <MeetingAudio
                  meetingId={task.meetingId}
                  startMs={s.startMs}
                  deleted={meeting?.audioDeleted || false}
                />
              </div>
            ))}
            {meeting && !sources.length && (
              <p role="alert" className="text-sm text-destructive">
                {t(
                  '原文引用缺失，请先打开来源会议检查或重新整理。',
                  'Cited excerpts are missing. Open the source meeting to check or regenerate the notes.',
                )}
              </p>
            )}
            {itemStatus(task.item) !== 'commitment' && (
              <p className="text-sm leading-6 text-muted-foreground">
                {t(
                  '原记录是提议或待确认，不能仅通过核对变成承诺。请在来源会议检查内容性质。',
                  'This is a proposal or pending item. Reviewing cannot turn it into a commitment; check its type in the source meeting.',
                )}
              </p>
            )}
            <label className="flex items-start gap-3 text-sm leading-6">
              <Checkbox
                className="mt-1"
                disabled={pending || !canConfirm}
                checked={edit.confirmed}
                onCheckedChange={(value) => setEdit({ ...edit, confirmed: value === true })}
              />
              {t(
                '我已对照原文，以上内容与原文一致',
                'I checked that these details match the source',
              )}
            </label>
            <p className="text-xs leading-6 text-muted-foreground">
              {t(
                '修改后未核对的内容会留在“需要确认”。缺少负责人或具体日期时，也会提示补充。',
                'Edited details remain in “Needs checking” until reviewed. Missing owners or calendar dates are flagged separately.',
              )}
            </p>
          </section>
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {meetingMessage(error, t)}
            </p>
          )}
        </div>
        <DialogFooter className="shrink-0 border-t pt-4">
          <Button variant="ghost" disabled={pending} onClick={openSource}>
            {t('打开来源会议', 'Open source meeting')}
          </Button>
          <Button variant="outline" disabled={pending} onClick={onClose}>
            {t('取消', 'Cancel')}
          </Button>
          <Button disabled={pending || !meeting || !edit.text.trim()} onClick={() => void save()}>
            {pending
              ? t('保存中…', 'Saving…')
              : edit.confirmed
                ? t('保存并确认原文', 'Save and confirm source')
                : t('保存修改', 'Save changes')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
