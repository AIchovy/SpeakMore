import { describe, expect, it, vi } from 'vitest'
import { renderToStaticMarkup } from 'react-dom/server'
import { asAppError, errorMessage } from './errors'
import { HistoryLoader } from './history-loader'
import { InlineError } from '@/components/common'

const locale = vi.hoisted(() => ({ english: true }))
vi.mock('./app-state', () => ({
  useText: () => (zh: string, en: string) => (locale.english ? en : zh),
}))
const en = (_zh: string, english: string) => english
const zh = (chinese: string) => chinese
describe('native error presentation', () => {
  it('keeps a failure structured so changing language also translates an existing alert', () => {
    const issue = asAppError({
      code: 'HashMismatch',
      userMessage: '文件校验失败，已保留之前的模型。',
      retryable: false,
    })
    locale.english = true
    const english = renderToStaticMarkup(<InlineError message={issue} />)
    expect(english).toContain('role="alert"')
    expect(english).toContain('File verification failed')
    expect(english).toContain('Previous model files have been kept')
    expect(english).not.toContain(issue.userMessage)
    locale.english = false
    expect(renderToStaticMarkup(<InlineError message={issue} />)).toContain(issue.userMessage)
    expect(issue.code).toBe('HashMismatch')
    locale.english = true
  })
  it('reports distinct next steps for common device, model, quota and persistence failures', () => {
    for (const [code, expected] of [
      ['MicrophonePermissionDenied', 'System Settings'],
      ['DeviceUnavailable', 'Choose a device'],
      ['ModelLoadTimeout', 'select another model'],
      ['ImportSelection', 'original filename'],
      ['InsufficientSpace', 'Free some space'],
      ['ShortcutConflict', 'Choose another combination'],
      ['UsageNotSaved', 'copy the text'],
      ['DeliveryReceiptNotSaved', 'check the target application'],
    ])
      expect(errorMessage({ code, userMessage: '原生中文' }, en)).toContain(expected)
  })
  it('does not echo unexpected exceptions, legacy strings, unknown codes or prototype keys', () => {
    for (const value of [
      new Error('private path and upstream response'),
      'private path and upstream response',
      { code: 'unknown', userMessage: 'private native diagnostic' },
      { code: '__proto__', userMessage: 'private prototype' },
      { code: 'constructor', userMessage: 'private constructor' },
      Object.create({ code: 'HashMismatch', userMessage: 'private inherited error' }),
      null,
    ]) {
      expect(asAppError(value).code).toBe('Unknown')
      expect(errorMessage(value, en)).not.toContain('private')
      expect(errorMessage(value, zh)).not.toContain('private')
    }
    expect(
      asAppError({ code: 'HashMismatch', userMessage: 'x'.repeat(4097) }).userMessage,
    ).not.toContain('xxx')
  })
  it('preserves a history error code until retry and never advances a failed page', async () => {
    let fail = true
    const requests: (string | null)[] = []
    const loader = new HistoryLoader(async (_query, cursor) => {
      requests.push(cursor)
      if (fail) throw { code: 'StorageUnavailable', userMessage: '本地数据不可用', retryable: true }
      return { items: [], nextCursor: null }
    })
    await loader.load()
    expect(loader.snapshot().error?.code).toBe('StorageUnavailable')
    expect(errorMessage(loader.snapshot().error, en)).toContain('disk space')
    expect(errorMessage(loader.snapshot().error, zh)).toBe('本地数据不可用')
    fail = false
    await loader.load()
    expect(loader.snapshot().error).toBeNull()
    expect(requests).toEqual([null, null])
  })
})
