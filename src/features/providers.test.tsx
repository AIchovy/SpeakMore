import { describe, expect, it, vi } from 'vitest'
import { renderToStaticMarkup } from 'react-dom/server'
import { ProviderRows } from './providers'
import { consumeCredential, type ProviderRow } from './providers-types'
import { providerErrorMessage } from './provider-errors'
import { ProviderRecoveryRows } from './provider-recovery'

vi.mock('@/lib/app-state', () => ({ useText: () => (zh: string) => zh, call: vi.fn() }))

const service: ProviderRow = {
  providerId: 'internal-provider-id',
  revision: 1,
  displayName: '<script>untrusted service</script>',
  protocol: { kind: 'ollama' },
  endpoint: 'http://127.0.0.1:11434/api/chat',
  model: 'local-model',
  local: true,
  credentialConfigured: false,
  destinationConfirmed: true,
  testedAt: null,
  testValid: false,
  preparedThisRun: false,
}
const render = (row: ProviderRow, disabled = false) =>
  renderToStaticMarkup(
    <ProviderRows
      rows={[row]}
      activeId={null}
      disabled={disabled}
      edit={() => {}}
      remove={() => {}}
      test={() => {}}
      select={() => {}}
    />,
  )

describe('provider configuration boundaries', () => {
  it('shows recovery actions without exposing native references or offering use and test', () => {
    const html = renderToStaticMarkup(
      <ProviderRecoveryRows
        disabled
        rows={[
          {
            recoveryId: 'native-review-token',
            displayName: '<script>broken</script>',
            kind: 'provider',
          },
          { recoveryId: 'another-native-token', displayName: null, kind: 'credential' },
        ]}
        review={() => {}}
      />,
    )
    expect(html).toContain('&lt;script&gt;broken&lt;/script&gt;')
    expect(html).not.toContain('<script>')
    expect(html).not.toContain('native-review-token')
    expect(html).not.toContain('another-native-token')
    expect(html).toContain('未自动删除')
    expect(html).toContain('忘记此记录')
    expect(html).not.toContain('测试连接')
    expect(html).not.toContain('使用</button>')
    for (const button of html.match(/<button\b[^>]*>[\s\S]*?<\/button>/g) ?? [])
      expect(button).toContain('disabled=""')
  })
  it('removes the typed key synchronously and never sends it for keep/remove actions', () => {
    for (const intent of ['keep', 'remove', 'replace'] as const) {
      const input = { value: 'synthetic-secret-only' }
      const payload = consumeCredential(intent, input)
      expect(input.value).toBe('')
      expect(payload).toEqual(
        intent === 'replace'
          ? { action: 'replace', key: 'synthetic-secret-only' }
          : { action: intent },
      )
    }
    expect(consumeCredential('remove', null)).toEqual({ action: 'remove' })
  })
  it('escapes service names and distinguishes a valid durable test from a timestamp or prepared cache', () => {
    const previous = render({ ...service, testedAt: 123 })
    expect(previous).toContain('&lt;script&gt;untrusted service&lt;/script&gt;')
    expect(previous).not.toContain('<script>')
    expect(previous).not.toContain('internal-provider-id')
    expect(previous).toContain('重新测试后可使用')
    const useButton = (html: string) =>
      html
        .match(/<button\b[^>]*>[\s\S]*?<\/button>/g)!
        .find((button) => button.endsWith('使用</button>'))!
    expect(useButton(previous)).toContain('disabled=""')
    const tested = { ...service, testedAt: 123, testValid: true, preparedThisRun: true }
    expect(useButton(render(tested))).not.toContain('disabled=""')
    expect(useButton(render(tested, true))).toContain('disabled=""')
    const restarted = render({ ...tested, preparedThisRun: false })
    expect(restarted).toContain('测试记录有效，使用时准备')
    expect(restarted).not.toContain('本次运行已准备')
    expect(useButton(restarted)).not.toContain('disabled=""')
  })
  it('keeps English errors actionable without showing raw upstream details', () => {
    const en = (_zh: string, english: string) => english
    expect(
      providerErrorMessage({ code: 'CredentialRebindRequired', userMessage: '地址已改变' }, en),
    ).toContain('Enter a new key')
    expect(providerErrorMessage({ code: 'RevisionConflict', userMessage: '冲突' }, en)).toContain(
      'reload',
    )
    expect(
      providerErrorMessage({ code: 'unknown', userMessage: 'private upstream body' }, en),
    ).not.toContain('private')
    expect(providerErrorMessage(null, en)).toBeNull()
  })
})
