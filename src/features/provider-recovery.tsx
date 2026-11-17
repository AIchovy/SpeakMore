import { Button } from '@/components/ui/button'
import { ShieldIcon } from '@/components/icons'
import { useText } from '@/lib/app-state'
import type { ProviderRecoveryRow } from './providers-types'

export function ProviderRecoveryRows({
  rows,
  disabled,
  review,
}: {
  rows: ProviderRecoveryRow[]
  disabled: boolean
  review: (row: ProviderRecoveryRow, trigger: HTMLButtonElement) => void
}) {
  const t = useText()
  if (!rows.length) return null
  return (
    <section className="space-y-3" aria-label={t('需要处理的配置', 'Settings needing attention')}>
      <h2 className="text-base font-semibold">
        {t('需要处理的配置', 'Settings needing attention')}
      </h2>
      <div className="divide-y rounded-xl border bg-card">
        {rows.map((row, index) => (
          <div key={row.recoveryId} className="flex flex-wrap items-center gap-4 p-5">
            <ShieldIcon aria-hidden className="size-5 shrink-0 text-muted-foreground" />
            <div className="min-w-0 flex-1 basis-60">
              <h3 className="break-words font-medium">
                {row.displayName ??
                  (row.kind === 'provider'
                    ? t(`无法读取的服务 ${index + 1}`, `Unreadable service ${index + 1}`)
                    : t('无法识别的凭据记录', 'Unrecognized credential record'))}
              </h3>
              <p className="mt-1 text-sm leading-6 text-muted-foreground">
                {row.kind === 'provider'
                  ? t(
                      '保存的配置无法使用。移除后可重新添加；其他服务仍可管理。',
                      'This saved configuration cannot be used. Remove it and add it again; other services remain available.',
                    )
                  : t(
                      '无法确认旧密钥的归属，未自动删除。请在系统钥匙串中检查 SpeakMore 项目。',
                      'The old key could not be identified and was not deleted automatically. Check SpeakMore entries in your system keychain.',
                    )}
              </p>
            </div>
            <Button
              variant="outline"
              size="sm"
              disabled={disabled}
              onClick={(event) => review(row, event.currentTarget)}
            >
              {row.kind === 'provider'
                ? t('移除配置', 'Remove configuration')
                : t('忘记此记录', 'Forget this record')}
            </Button>
          </div>
        ))}
      </div>
    </section>
  )
}
