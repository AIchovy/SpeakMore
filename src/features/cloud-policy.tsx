import { useState } from 'react'
import { useApp, useText } from '@/lib/app-state'
import { providerErrorMessage } from './provider-errors'
import { SettingsRow, InlineError } from '@/components/common'
import { Switch } from '@/components/ui/switch'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from '@/components/ui/dialog'

export function CloudPolicy() {
  const { data, save, pending } = useApp(),
    t = useText()
  const [confirm, setConfirm] = useState(false),
    [error, setError] = useState<unknown>(null)
  if (!data) return null
  async function change(allow: boolean) {
    setError(null)
    try {
      await save({ processingPolicy: allow ? 'allow_cloud' : 'local_only' })
      setConfirm(false)
    } catch (error) {
      setError(error)
    }
  }
  return (
    <>
      <SettingsRow
        title={t('允许云端文字处理', 'Allow cloud text processing')}
        description={t(
          '仅向你确认的服务发送文字。关闭后，只允许本机服务。',
          'Send text only to services you confirm. Turn off to allow local services only.',
        )}
      >
        <Switch
          aria-label={t('允许云端文字处理', 'Allow cloud text processing')}
          checked={data.settings.processingPolicy === 'allow_cloud'}
          disabled={pending}
          onCheckedChange={(allow) => (allow ? setConfirm(true) : void change(false))}
        />
      </SettingsRow>
      {!confirm && <InlineError message={providerErrorMessage(error, t)} />}
      <Dialog
        open={confirm}
        onOpenChange={(open) => {
          if (!pending) {
            setConfirm(open)
            setError(null)
          }
        }}
      >
        <DialogContent closeLabel={t('关闭', 'Close')}>
          <DialogHeader>
            <DialogTitle>
              {t('允许发送文字到已确认的服务', 'Allow text to confirmed services')}
            </DialogTitle>
            <DialogDescription>
              {t(
                '文字会发送到你配置并确认的服务地址，音频仍在本机处理。服务商可能按其政策处理或保留文字；取消不能撤回已经发送的内容。',
                'Text is sent to the service addresses you configure and confirm. Audio stays on this device. Providers may process or retain text under their policies; cancelling cannot recall text already sent.',
              )}
            </DialogDescription>
          </DialogHeader>
          <p className="text-sm leading-6 text-muted-foreground">
            {t(
              '当前仅可发送固定测试句。文字整理可用后，仍需由你单独开启。',
              'Currently this allows a fixed connection-test sentence. Text polishing will still need to be enabled separately when available.',
            )}
          </p>
          <InlineError message={providerErrorMessage(error, t)} />
          <DialogFooter>
            <Button variant="outline" disabled={pending} onClick={() => setConfirm(false)}>
              {t('取消', 'Cancel')}
            </Button>
            <Button disabled={pending} onClick={() => void change(true)}>
              {t('允许云端文字处理', 'Allow cloud text processing')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
