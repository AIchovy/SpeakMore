import { useState } from 'react'
import { useApp, useText } from '@/lib/app-state'
import { Button } from '@/components/ui/button'
import { meetingError } from './meetings-api'
import { meetingMessage } from './meeting-messages'

export function MeetingMicrophoneAccess({
  source,
  denied,
  disabled,
  onRecheck,
}: {
  source: string
  denied: boolean
  disabled: boolean
  onRecheck: () => void
}) {
  const { data, mutate, refresh } = useApp()
  const t = useText()
  const [pending, setPending] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [checked, setChecked] = useState(false)
  if (
    source === 'system' ||
    (!denied && (!data?.desktop || data.permissions.microphone === 'authorized'))
  )
    return null
  const microphone = data?.permissions.microphone
  const waiting = data?.permissions.requestingMicrophone
  const act = async (recheck: boolean) => {
    setPending(true)
    setError(null)
    setChecked(false)
    try {
      if (recheck) {
        await refresh()
        onRecheck()
        setChecked(true)
      } else {
        await mutate('microphone_permission_action', { request: microphone === 'not_determined' })
      }
    } catch (failure) {
      setError(meetingError(failure))
    } finally {
      setPending(false)
    }
  }
  return (
    <section
      aria-label={t('会议麦克风权限', 'Meeting microphone access')}
      className="space-y-3 rounded-lg border bg-muted/30 p-4 text-sm"
    >
      <p className="font-medium">{t('先允许麦克风访问', 'Allow microphone access first')}</p>
      <p className="leading-6 text-muted-foreground">
        {t(
          '当前音源需要麦克风权限。授权前会议不会开始，也不会创建空记录。授权后请再次点击“开始录音”。',
          'This audio source needs microphone access. The meeting will not start or create an empty record before access is allowed. Click “Start recording” again after allowing access.',
        )}
      </p>
      {microphone !== 'not_determined' && (
        <p className="text-xs leading-5 text-muted-foreground">
          {t(
            '系统设置 → 隐私与安全性 → 麦克风 → SpeakMore。',
            'System Settings → Privacy & Security → Microphone → SpeakMore.',
          )}
        </p>
      )}
      {microphone === 'restricted' && (
        <p className="text-xs text-muted-foreground">
          {t(
            '访问受到系统限制，请联系设备管理员。',
            'Access is restricted by the system. Contact your device administrator.',
          )}
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        <Button
          variant="outline"
          size="sm"
          disabled={disabled || pending || waiting}
          onClick={() => void act(false)}
        >
          {waiting
            ? t('等待系统授权', 'Waiting for your choice')
            : microphone === 'not_determined'
              ? t('授权麦克风', 'Allow microphone')
              : t('打开系统设置', 'Open System Settings')}
        </Button>
        <Button
          variant="ghost"
          size="sm"
          disabled={disabled || pending || waiting}
          onClick={() => void act(true)}
        >
          {t('重新检查权限', 'Check access again')}
        </Button>
      </div>
      {checked && (
        <p role="status" className="text-xs text-muted-foreground">
          {t(
            '已重新检查，麦克风访问尚未获准。',
            'Checked again: microphone access is still not allowed.',
          )}
        </p>
      )}
      {error && (
        <p role="alert" className="text-destructive">
          {meetingMessage(error, t)}
        </p>
      )}
    </section>
  )
}
