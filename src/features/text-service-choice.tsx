import { meetingMessage } from './meeting-messages'
import { useText } from '@/lib/app-state'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { useEffect, useId, useState } from 'react'
import { Link } from 'react-router-dom'
import { call, useApp } from '@/lib/app-state'
import { Button } from '@/components/ui/button'
import { meetingCall, meetingError, type Meeting } from './meetings-api'
import type { ProviderRow, ProvidersView } from './providers-types'

type Preference = { providerId: string | null; followAi: boolean }
const FOLLOW = 'follow_ai'
const LOCAL = 'local_qwen'
export function TextServiceChoice({
  meeting,
  disabled,
  purpose = 'meeting',
  onChange,
  onSelectionChange,
}: {
  meeting?: Meeting
  disabled?: boolean
  purpose?: 'meeting' | 'captions'
  onChange?: (effectiveProviderId: string | null) => void
  onSelectionChange?: () => void
}) {
  const t = useText()

  const { data } = useApp()
  const id = useId()
  const [rows, setRows] = useState<ProviderRow[]>([])
  const [selected, setSelected] = useState(FOLLOW)
  const [pending, setPending] = useState(false)
  const [loaded, setLoaded] = useState(false)
  const [error, setError] = useState('')
  const [attempt, setAttempt] = useState(0)
  const activeId = data?.settings.activeProviderId ?? null
  useEffect(() => {
    let live = true
    setLoaded(false)
    setError('')
    Promise.all([
      call<ProvidersView>('get_providers'),
      meeting
        ? Promise.resolve({
            providerId: meeting.summaryProviderId,
            followAi: meeting.summaryFollowAi,
          })
        : meetingCall<Preference>('meeting_summary_settings', { purpose }),
    ])
      .then(([providers, preference]) => {
        if (!live) return
        setRows(providers.rows)
        setSelected(preference.followAi ? FOLLOW : (preference.providerId ?? ''))
        setLoaded(true)
        onChange?.(preference.followAi ? activeId : (preference.providerId ?? null))
      })
      .catch((e) => {
        if (live) setError(meetingError(e))
      })
    return () => {
      live = false
    }
    // The callback only informs the parent; it is not a resource dependency.
  }, [
    meeting?.id,
    meeting?.summaryProviderId,
    meeting?.summaryFollowAi,
    activeId,
    purpose,
    attempt,
  ])
  const effectiveId = selected === FOLLOW ? activeId : selected || null
  const row = rows.find((r) => r.providerId === effectiveId)
  const activeRow = rows.find((r) => r.providerId === activeId)
  const change = async (value: string) => {
    setPending(true)
    setError('')
    try {
      await meetingCall('meeting_summary_choose', {
        providerId: value === FOLLOW ? null : value || null,
        followAi: value === FOLLOW,
        purpose,
        meetingId: meeting?.id ?? null,
        revision: meeting?.revision ?? null,
      })
      setSelected(value)
      onChange?.(value === FOLLOW ? activeId : value || null)
      onSelectionChange?.()
    } catch (e) {
      setError(meetingError(e))
    } finally {
      setPending(false)
    }
  }
  return (
    <div className="my-5 border-y py-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <label htmlFor={id} className="text-sm font-medium">
          {purpose === 'captions'
            ? t('字幕翻译服务', 'Caption translation service')
            : meeting
              ? t('本次会议 AI 服务', 'AI service for this meeting')
              : t('新会议的 AI 服务', 'AI service for new meetings')}
        </label>
        <Select
          value={selected || LOCAL}
          disabled={disabled || pending || !loaded}
          onValueChange={(value) => void change(value === LOCAL ? '' : value)}
        >
          <SelectTrigger id={id} className="w-full max-w-full sm:w-[300px]">
            <SelectValue />
          </SelectTrigger>
          <SelectContent position="popper" align="end">
            <SelectItem value={FOLLOW}>
              {t('跟随当前 AI 服务', 'Use current AI service')}
              {activeRow ? ` · ${activeRow.displayName}` : ''}
            </SelectItem>
            <SelectItem value={LOCAL}>{t('本地 Qwen3 · 离线', 'Local Qwen3 · Offline')}</SelectItem>
            {selected !== FOLLOW && selected && !row && (
              <SelectItem value={selected} disabled>
                {t('原服务已不可用，请重新选择', 'Previous service unavailable. Choose another.')}
              </SelectItem>
            )}
            {rows
              .filter((r) => r.providerId !== activeId || selected === r.providerId)
              .map((r) => (
                <SelectItem
                  key={r.providerId}
                  value={r.providerId}
                  disabled={
                    !r.testValid || (!r.local && data?.settings.processingPolicy !== 'allow_cloud')
                  }
                >
                  {r.displayName} · {r.model}
                  {r.providerId === activeId ? t(' · 单独指定', ' · Explicit selection') : ''}
                  {!r.testValid
                    ? t(' · 需测试连接', ' · Connection test needed')
                    : !r.local && data?.settings.processingPolicy !== 'allow_cloud'
                      ? t(' · 需允许云端', ' · Cloud access needed')
                      : ''}
                </SelectItem>
              ))}
          </SelectContent>
        </Select>
      </div>
      <p className="mt-2 break-words text-xs leading-6 text-muted-foreground">
        {!effectiveId
          ? selected === FOLLOW
            ? t(
                '未选择当前 AI 服务，使用本地 Qwen3。录音与文字均在本机处理。',
                'No current AI service selected. Using local Qwen3; audio and text are processed on your device.',
              )
            : t('录音与文字均在本机处理。', 'Audio and text are processed on your device.')
          : row
            ? t(
                `音频在本机转写；${purpose === 'captions' ? '字幕及助手对话' : '会议原文及助手对话'}将发送到 ${row.endpoint}。${row.local ? '由你配置的本机服务处理。' : '服务商可能按其政策保留文字。'}`,
                `Audio is transcribed on your device; ${purpose === 'captions' ? 'captions and assistant conversations' : 'meeting transcripts and assistant conversations'} are sent to ${row.endpoint}. ${row.local ? 'Processed by your configured local service.' : 'The provider may retain text under its policies.'}`,
              )
            : t(
                '当前服务不可用，不会自动切换到其他服务。',
                'The selected service is unavailable. No other service will be used automatically.',
              )}{' '}
        <Link className="underline underline-offset-4" to="/settings/ai">
          {t('管理 AI 服务', 'Manage AI services')}
        </Link>
      </p>
      {meeting?.summaryStale && (
        <p className="mt-1 text-xs text-muted-foreground">
          {t(
            '已保留现有纪要，重新整理后纳入最新内容与设置。',
            'Existing notes are kept. Regenerate to include the latest content and settings.',
          )}
        </p>
      )}
      {error && (
        <p role="alert" className="mt-2 text-xs text-destructive">
          {meetingMessage(error, t)}
          {!loaded && (
            <Button
              variant="link"
              size="xs"
              className="ml-2 h-auto p-0 text-xs"
              onClick={() => setAttempt((n) => n + 1)}
            >
              {t('重试', 'Retry')}
            </Button>
          )}
        </p>
      )}
    </div>
  )
}
