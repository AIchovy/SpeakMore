import { describe, expect, it, vi } from 'vitest'
import { renderToStaticMarkup } from 'react-dom/server'
import { RuleRows } from './app-rules'
vi.mock('@/lib/app-state', () => ({ useText: () => (zh: string) => zh, call: vi.fn() }))
describe('app rule presentation', () => {
  it('distinguishes inherited settings from exclusions without exposing identity or raw markup', () => {
    const html = renderToStaticMarkup(
      <RuleRows
        items={[
          {
            platform: 'macos',
            appIdentity: 'private.internal.identity',
            displayName: '<script>Notes</script>',
            manualOnly: false,
            historyExcluded: true,
          },
        ]}
        disabled={false}
        edit={() => {}}
        remove={() => {}}
      />,
    )
    expect(html).toContain('跟随全局交付设置')
    expect(html).toContain('不保存历史')
    expect(html).toContain('&lt;script&gt;Notes&lt;/script&gt;')
    expect(html).not.toContain('<script>')
    expect(html).not.toContain('private.internal.identity')
  })
})
