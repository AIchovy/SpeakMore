import { asAppError, type UiMessage } from '@/lib/errors'
import { useCallback, useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import { call, useApp, useText } from '@/lib/app-state'
import { errorMessage, type InputDevice } from '@/lib/types'
import { Button } from '@/components/ui/button'
import { Choice, InlineError, SettingsRow } from '@/components/common'
import { ReloadIcon } from '@/components/icons'

export function InputDevices() {
  const { data, mutate, pending } = useApp()
  const t = useText()
  const [devices, setDevices] = useState<InputDevice[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<UiMessage | null>(null)
  const generation = useRef(0)
  const desktop = data?.desktop ?? false
  const refresh = useCallback(async () => {
    if (!desktop) return
    const current = ++generation.current
    setLoading(true)
    setError(null)
    try {
      const next = await call<InputDevice[]>('list_input_devices')
      if (current === generation.current) setDevices(next)
    } catch (failure) {
      if (current === generation.current) setError(asAppError(failure))
    } finally {
      if (current === generation.current) setLoading(false)
    }
  }, [desktop])
  useEffect(() => {
    void refresh()
    return () => {
      generation.current++
    }
  }, [refresh])
  if (!data) return null
  const selected = data.settings.inputDeviceId
  const missing =
    selected !== null && !loading && !error && !devices.some((device) => device.id === selected)
  const options: [string, string][] = [
    ['system-default', t('跟随系统默认', 'Use system default')],
    ...devices.map(
      (device) =>
        [
          device.id,
          device.name + (device.configuration ? '' : t('（格式不支持）', ' (unsupported format)')),
        ] as [string, string],
    ),
  ]
  if (selected && !devices.some((device) => device.id === selected))
    options.push([selected, t('先前选择的设备', 'Previously selected device')])
  return (
    <>
      <SettingsRow
        title={t('麦克风', 'Microphone')}
        description={t(
          '每次录音使用所选设备。录音中断开时，本次录音会结束。',
          'Use this microphone for each recording. Disconnecting it ends the current recording.',
        )}
      >
        <div className="flex max-w-full items-center gap-2">
          <Choice
            label={t('选择麦克风', 'Choose microphone')}
            value={selected ?? 'system-default'}
            options={options}
            disabled={!desktop || loading || pending || data.session.phase !== 'idle'}
            className="w-[220px] max-w-full"
            onChange={(value) => {
              void mutate('set_input_device', {
                expectedRevision: data.settings.revision,
                deviceId: value === 'system-default' ? null : value,
              }).catch((failure) => toast.error(errorMessage(failure, t)))
            }}
          />
          <Button
            variant="ghost"
            size="icon"
            aria-label={t('刷新麦克风列表', 'Refresh microphones')}
            disabled={!desktop || loading}
            onClick={() => void refresh()}
          >
            <ReloadIcon className="size-4" />
          </Button>
        </div>
      </SettingsRow>
      {!desktop && (
        <p className="pt-2 text-sm text-muted-foreground">
          {t(
            '请在桌面应用中选择麦克风。浏览器预览无法读取系统设备。',
            'Choose a microphone in the desktop app. Browser preview cannot read system devices.',
          )}
        </p>
      )}
      {loading && (
        <p role="status" className="pt-2 text-sm text-muted-foreground">
          {t('正在读取麦克风…', 'Reading microphones…')}
        </p>
      )}
      {error && <InlineError message={error} />}
      {desktop && !loading && !error && (missing || devices.length === 0) && (
        <p role="status" className="pt-2 text-sm text-muted-foreground">
          {t(
            '未找到所选麦克风。请连接设备或选择系统默认，然后刷新。',
            'The selected microphone is unavailable. Connect it or choose the system default, then refresh.',
          )}
        </p>
      )}
    </>
  )
}
