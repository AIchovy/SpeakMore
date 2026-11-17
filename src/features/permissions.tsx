import { asAppError, type UiMessage } from '@/lib/errors'
import { useState } from 'react'
import { useApp, useText } from '@/lib/app-state'
import { Button } from '@/components/ui/button'
import { InlineError, SettingsRow } from '@/components/common'

export function Permissions() {
  const { data, mutate, requestPermissionRestart } = useApp(),
    t = useText()
  const [pending, setPending] = useState(false)
  const [error, setError] = useState<UiMessage | null>(null)
  const [checkedAccessibility, setCheckedAccessibility] = useState<boolean | null>(null)
  if (!data) return null
  const { microphone, accessibility, requestingMicrophone } = data.permissions
  const labels = {
    not_determined: t('尚未授权', 'Not requested'),
    denied: t('访问已关闭', 'Access denied'),
    restricted: t('受到系统限制', 'Restricted by the system'),
    authorized: t('已授权', 'Allowed'),
    unavailable: t('无法确认', 'Unavailable'),
  }
  async function act(request: boolean) {
    setPending(true)
    setError(null)
    try {
      await mutate('microphone_permission_action', { request })
    } catch (failure) {
      setError(asAppError(failure))
    } finally {
      setPending(false)
    }
  }
  async function openAccessibility() {
    setPending(true)
    setError(null)
    try {
      await mutate('open_accessibility_settings', {})
    } catch (failure) {
      setError(asAppError(failure))
    } finally {
      setPending(false)
    }
  }
  async function recheckAccessibility() {
    setPending(true)
    setError(null)
    try {
      setCheckedAccessibility(await mutate<boolean>('check_accessibility_permission', {}))
    } catch (failure) {
      setError(asAppError(failure))
    } finally {
      setPending(false)
    }
  }
  return (
    <section aria-label={t('系统权限', 'System permissions')}>
      <SettingsRow
        title={t('麦克风权限', 'Microphone access')}
        description={t(
          '只在你主动听写时采集声音，音频在设备上处理。授权后仍需手动开始录音。',
          'Capture sound only when you start dictation. Audio is processed on your device. Allowing access does not start a recording.',
        )}
      >
        <div className="flex flex-wrap items-center justify-end gap-3">
          <span role="status" className="text-sm text-muted-foreground">
            {!data.desktop
              ? t('请使用 Mac 应用', 'Use the Mac app')
              : requestingMicrophone
                ? t('等待系统授权', 'Waiting for your choice')
                : labels[microphone]}
          </span>
          {data.desktop && microphone !== 'authorized' && (
            <Button
              variant="outline"
              disabled={
                pending ||
                requestingMicrophone ||
                data.captureSuspended ||
                data.session.sessionId !== null
              }
              onClick={() => void act(microphone === 'not_determined')}
            >
              {microphone === 'not_determined'
                ? t('授权麦克风', 'Allow microphone')
                : t('打开系统设置', 'Open System Settings')}
            </Button>
          )}
        </div>
      </SettingsRow>
      {error && <InlineError message={error} />}
      {data.desktop && microphone === 'restricted' && (
        <p className="mt-2 text-sm text-muted-foreground">
          {t(
            '设备管理或家长控制可能限制了访问，请联系设备管理员。',
            'Device management or parental controls may restrict access. Contact your device administrator.',
          )}
        </p>
      )}
      {data.desktop && ['denied', 'unavailable'].includes(microphone) && (
        <p className="mt-2 text-sm text-muted-foreground">
          {t(
            '系统设置 → 隐私与安全性 → 麦克风。修改后会重新检查；若系统要求重新启动，请先保存其他工作。',
            'System Settings → Privacy & Security → Microphone. Access is checked again after changes; restart if macOS requests it.',
          )}
        </p>
      )}
      <SettingsRow
        title={t('辅助功能权限', 'Accessibility access')}
        description={t(
          'Fn 全局快捷键和跨应用自动输入需要此权限。未授权仍可在主窗口听写并复制。',
          'Required for the global Fn shortcut and automatic insertion in other apps. You can still dictate and copy from the main window without it.',
        )}
      >
        <div className="flex flex-wrap items-center justify-end gap-3">
          <span className="text-sm text-muted-foreground">
            {!data.desktop
              ? t('桌面应用中查看', 'Check in the desktop app')
              : accessibility
                ? t('已授权', 'Allowed')
                : t('系统尚未确认授权 · Fn 不可用', 'Access not confirmed · Fn unavailable')}
          </span>
          {data.desktop && !accessibility && (
            <Button
              variant="outline"
              disabled={pending || !!data.session.sessionId}
              onClick={() => void openAccessibility()}
            >
              {t('打开辅助功能设置', 'Open Accessibility settings')}
            </Button>
          )}
        </div>
      </SettingsRow>
      {data.desktop && !accessibility && (
        <div className="mt-3 space-y-3 rounded-lg border bg-muted/30 p-4 text-sm">
          <p className="leading-6 text-muted-foreground">
            {t(
              '在系统设置中开启 SpeakMore 后，请重启应用以应用授权，无需重新保存快捷键。',
              'After allowing SpeakMore in System Settings, restart the app to apply access. You do not need to save your shortcut again.',
            )}
          </p>
          <div className="flex flex-wrap gap-2">
            <Button
              variant="outline"
              disabled={pending}
              onClick={() => void recheckAccessibility()}
            >
              {t('重新检查权限', 'Check access again')}
            </Button>
            <Button
              variant="outline"
              disabled={pending || !!data.session.sessionId || data.captureSuspended}
              onClick={requestPermissionRestart}
            >
              {t('重启 SpeakMore', 'Restart SpeakMore')}
            </Button>
          </div>
          {checkedAccessibility === false && (
            <p role="status" className="text-xs text-muted-foreground">
              {t(
                '已重新检查：macOS 尚未授予当前运行的 SpeakMore 辅助功能权限。',
                'Checked again: macOS has not granted Accessibility access to this running copy of SpeakMore.',
              )}
            </p>
          )}
        </div>
      )}
    </section>
  )
}
