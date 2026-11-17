import { expect, it, vi } from 'vitest'
import { renderToStaticMarkup } from 'react-dom/server'
import { StorageRecovery, StorageBackupNotice, recoverySteps } from './storage-recovery'
import { createPreview } from '@/lib/preview'
vi.mock('@/lib/app-state', () => ({ call: vi.fn() }))

it('explains distinct recovery actions and keeps untrusted errors inert', () => {
  const html = renderToStaticMarkup(
    <StorageRecovery
      status={{
        error: {
          code: 'DatabaseMigrationFailed',
          userMessage: '<script>private</script>',
          retryable: false,
        },
        cleanupPending: false,
        canReveal: false,
      }}
    />,
  )
  expect(html).toContain('&lt;script&gt;private&lt;/script&gt;')
  expect(html).not.toContain('<script>')
  expect(html).toContain('修复后的应用版本')
  expect(html).toContain('disabled=""')
  expect(html).not.toContain('已成功恢复')
  expect(recoverySteps('NewerDatabase').join('')).toContain('同版或更新版本')
  expect(recoverySteps('DatabaseBackupFailed').join('')).toContain('可用空间')
  expect(recoverySteps('DatabaseCorrupt').join('')).toContain('日志文件')
  expect(recoverySteps('NewerDatabase', true).join('')).toContain('same or a newer')
  expect(
    renderToStaticMarkup(
      <StorageRecovery
        initialEnglish
        status={{
          error: { code: 'DatabaseMigrationFailed', userMessage: '原文件保留', retryable: false },
          cleanupPending: false,
          canReveal: true,
        }}
      />,
    ),
  ).toContain('Original files have been kept')
})

it('discloses retained history backups and never simulates native recovery in a browser', async () => {
  const html = renderToStaticMarkup(
    <StorageBackupNotice status={{ error: null, cleanupPending: true, canReveal: true }} />,
  )
  expect(html).toContain('备份可能包含历史记录')
  expect(html).toContain('下次启动会重试清理')
  const preview = createPreview()
  expect(await preview('get_storage_status')).toEqual({
    error: null,
    cleanupPending: false,
    canReveal: false,
  })
  for (const command of ['reveal_recovery_data', 'restart_after_storage_error'])
    await expect(preview(command)).rejects.toMatchObject({ code: 'DesktopRequired' })
})
