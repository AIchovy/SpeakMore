import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { RecordingBar } from './recording-bar'
import type { RecordingStatus } from '@/lib/types'

const base: RecordingStatus = {
  revision: 1,
  session: {
    sessionId: 'session-a',
    revision: 1,
    phase: 'recording',
    mode: 'hold',
    elapsedMs: 0,
    maxDurationMs: 300_000,
  },
  audioLevel: 0.4,
  uiLocale: 'zh-CN',
  motionPolicy: 'system',
  preview: false,
  notice: null,
}
const render = (patch: Partial<RecordingStatus>) =>
  renderToStaticMarkup(<RecordingBar status={{ ...base, ...patch }} act={async () => {}} />)
describe('recording bar state contract', () => {
  it('shows the actual automatic insertion fallback reason on the bar', () => {
    const notice = {
      sessionId: 'session-a',
      hasResult: true,
      error: null,
      deliveryStatus: 'ready_for_copy',
      deliveryReason: 'target_changed',
    }
    expect(render({ notice })).toContain('输入位置已变化')
    expect(render({ notice })).toContain('打开应用后复制')
    expect(render({ notice })).not.toContain('已提交粘贴')
    expect(render({ uiLocale: 'en', notice })).toContain('Input target changed')
  })
  it('distinguishes paste submission, clipboard fallback and uncertain delivery', () => {
    const notice = { sessionId: 'session-a', hasResult: true, error: null }
    const submitted = render({ notice: { ...notice, deliveryStatus: 'submitted' } })
    expect(submitted).toContain('已提交粘贴')
    expect(submitted).toContain('请在原输入框查看文字')
    expect(submitted).not.toContain('打开应用后复制')
    const copied = render({ notice: { ...notice, deliveryStatus: 'copied_only' } })
    expect(copied).toContain('在目标输入框按 ⌘V')
    expect(copied).not.toContain('已提交粘贴')
    const uncertain = render({ notice: { ...notice, deliveryStatus: 'uncertain' } })
    expect(uncertain).toContain('请确认是否已输入，避免重复')
    expect(uncertain).not.toContain('已提交粘贴')
  })
  it('explains the remaining time and processing after the complete five-minute limit', () => {
    expect(render({ session: { ...base.session, elapsedMs: 290_000 } })).toContain('还可录音 10 秒')
    const processing = render({
      session: { ...base.session, elapsedMs: 300_000, phase: 'transcribing' },
    })
    expect(processing).toContain('已达 5 分钟，正在转写')
    expect(processing).toContain('aria-label="取消听写"')
    expect(processing).toContain('05:00')
    expect(processing).not.toContain('已提交')
  })
  it('offers no start action from a microphone-free preview', () => {
    const preview = render({
      preview: true,
      session: { ...base.session, sessionId: null, phase: 'idle' },
    })
    expect(preview).toContain('关闭预览')
    expect(preview).toContain('disabled=""')
    expect(preview).not.toContain('开始录音')
  })
  it('maps normal speech onto the dBFS meter instead of the floor', () => {
    const scales = (audioLevel: number) =>
      [...render({ audioLevel }).matchAll(/scaleY\(([\d.]+)\)/g)].map((m) => Number(m[1]))
    expect(scales(0)).toEqual([0.13, 0.13, 0.13, 0.13, 0.13])
    // RMS 0.03 is ordinary speech; the linear meter kept the tallest bar near 0.13.
    expect(Math.max(...scales(0.03))).toBeGreaterThan(0.5)
    expect(Math.max(...scales(1))).toBe(1)
  })
  it('renders reduced motion with a static level symbol', () => {
    expect(render({ motionPolicy: 'reduced' })).not.toContain('recording-level')
    expect(render({ motionPolicy: 'reduced' })).toContain('正在听写')
  })
  it('keeps error content inert and never claims delivery for a manual result', () => {
    const error = render({
      notice: {
        sessionId: 'session-a',
        hasResult: false,
        error: { code: 'NoSpeech', userMessage: '<img src=x onerror=alert(1)>', retryable: true },
      },
    })
    expect(error).not.toContain('<img')
    expect(error).toContain('&lt;img')
    const result = render({ notice: { sessionId: 'session-a', hasResult: true, error: null } })
    expect(result).toContain('结果已保留')
    expect(result).toContain('打开应用后复制')
    expect(result).not.toContain('已提交')
  })
  it('translates a retained native failure and keeps recovery available in English', () => {
    const notice = {
      sessionId: 'session-a',
      hasResult: false,
      error: { code: 'NoSpeech', userMessage: '未检测到清晰语音，请重新录音。', retryable: true },
    }
    const english = render({ uiLocale: 'en', notice })
    expect(english).toContain('No clear speech was detected')
    expect(english).toContain('Open app for details')
    expect(english).not.toContain(notice.error.userMessage)
    expect(english).not.toContain('Result is ready')
    expect(render({ notice })).toContain(notice.error.userMessage)
    expect(
      render({
        uiLocale: 'en',
        notice: {
          ...notice,
          error: { ...notice.error, code: 'unrecognized', userMessage: 'private diagnostic' },
        },
      }),
    ).not.toContain('private diagnostic')
  })
})
