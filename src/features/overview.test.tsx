import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderToStaticMarkup } from 'react-dom/server'
import { MemoryRouter } from 'react-router-dom'
import type { Snapshot } from '@/lib/types'
import { createPreview } from '@/lib/preview'
import { Overview } from './overview'

const state = vi.hoisted(() => ({ data: null as Snapshot | null, english: false }))
vi.mock('@/lib/app-state', () => ({
  useApp: () => ({ data: state.data, pending: false, mutate: vi.fn(), save: vi.fn() }),
  useLiveSession: () => ({
    elapsedMs: state.data!.session.elapsedMs,
    audioLevel: state.data!.audioLevel,
  }),
  useText: () => (zh: string, en: string) => (state.english ? en : zh),
  call: vi.fn(),
}))
const render = () =>
  renderToStaticMarkup(
    <MemoryRouter>
      <Overview />
    </MemoryRouter>,
  )
beforeEach(async () => {
  state.data = (await createPreview()('get_snapshot')) as Snapshot
  state.english = false
})

describe('dictation controls and result rendering', () => {
  it('shows the saved refinement mode only when enabled, including next-session changes', () => {
    const preferences = state.data!.settings
    for (const [mode, zh, en] of [
      ['standard', '标准整理', 'Standard'],
      ['email', '邮件正文', 'Email'],
      ['code_comment', '代码注释', 'Code comments'],
      ['custom', '自定义', 'Custom'],
    ] as const) {
      preferences.polishMode = mode
      preferences.polishEnabled = true
      state.english = false
      expect(render()).toContain('按应用与网站整理')
      expect(render()).not.toContain('保持原文')
      state.english = true
      expect(render()).toContain('Adapt to app and website')
      expect(render()).not.toContain('Original wording')
      preferences.polishEnabled = false
      expect(render()).toContain('Original wording')
    }
    state.english = false
    preferences.polishEnabled = true
    preferences.polishMode = 'email'
    Object.assign(state.data!.session, { sessionId: 'active-session', phase: 'recording' })
    const active = render()
    expect(active).toContain('下次听写的文字整理')
    expect(active).toContain('按应用与网站整理')
    preferences.polishEnabled = false
    expect(render()).toContain('保持原文')
    expect(render()).not.toContain('下次听写的文字整理')
  })
  it('localizes native session failures and persistence warnings without claiming data was saved', () => {
    state.english = true
    Object.assign(state.data!, {
      lastError: { code: 'AsrTimeout', userMessage: '原生转写失败提示', retryable: true },
      storageWarning: { code: 'UsageNotSaved', userMessage: '原生存储警告', retryable: true },
    })
    const html = render()
    expect(html).toContain('Transcription took too long')
    expect(html).toContain('Local usage could not be saved')
    expect(html).toContain('copy the text from recent results')
    expect(html).not.toContain('原生转写失败提示')
    expect(html).not.toContain('原生存储警告')
    expect(html).not.toContain('Start dictation</button>')
    state.english = false
    expect(render()).toContain('原生转写失败提示')
    expect(render()).toContain('原生存储警告')
  })
  it('routes device blockers to audio settings and allows use beyond twenty results', () => {
    state.data!.desktop = true
    for (const action of ['microphone', 'device'] as const) {
      state.data!.readinessAction = action
      const html = render()
      expect(html).toContain('href="/settings/audio"')
      expect(html).not.toContain('管理模型')
    }
    state.data!.readinessAction = 'none'
    state.data!.readinessCode = 'ready'
    state.data!.usageToday = 25
    state.data!.canStart = true
    expect(render()).toContain('开始听写</button>')
  })
  it('keeps finish and cancel available near the five-minute limit', () => {
    Object.assign(state.data!.session, {
      sessionId: 'test',
      phase: 'recording',
      elapsedMs: 299_000,
    })
    const html = render()
    expect(html).toContain('04:59')
    expect(html).toContain('05:00')
    expect(html).toContain('完成听写</button>')
    expect(html).toContain('取消</button>')
    expect(html).toContain('届时自动完成')
  })
  it('shows ordinary speech levels on the meter with transforms only', () => {
    Object.assign(state.data!.session, { sessionId: 'test', phase: 'recording' })
    state.data!.audioLevel = 0.03
    const html = render()
    const scales = [...html.matchAll(/scaleY\(([\d.]+)\)/g)].map((m) => Number(m[1]))
    expect(scales).toHaveLength(13)
    // The tallest bar reaches about 60% of its height for RMS 0.03 speech.
    expect(Math.max(...scales)).toBeGreaterThan(0.5)
    expect(html).not.toMatch(/style="opacity:[^"]*height/)
  })
  it('allows cancellation during transcription but cannot stop the same recording twice', () => {
    Object.assign(state.data!.session, {
      sessionId: 'test',
      phase: 'transcribing',
      elapsedMs: 300_000,
    })
    const html = render()
    expect(html).toContain('正在转写')
    expect(html).toContain('取消</button>')
    expect(html).not.toContain('完成听写</button>')
    expect(html).not.toContain('开始听写</button>')
  })
  it('renders model output as inert text and exposes explicit copying', () => {
    state.data!.recentResults = [
      {
        sessionId: 'test',
        rawText: '<script>alert("test")</script><img src=x onerror=alert(1)>',
        polishedText: null,
        modelId: 'test',
        modelSha256: 'a'.repeat(64),
        dictionaryRevision: 1,
        language: 'zh',
        durationMs: 300_000,
        needsReview: true,
        asrMs: 1000,
        polishMs: null,
        polishOutcome: 'off',
        deliveryStatus: 'ready_for_copy',
        deliveryReason: null,
        appIdentity: null,
        createdAt: 1_700_000_000_000,
      },
    ]
    const html = render()
    expect(html).toContain('&lt;script&gt;')
    expect(html).not.toContain('<script>')
    expect(html).not.toContain('<img src=x')
    expect(html).toContain('复制</button>')
    expect(html).toContain('请检查后使用')
    expect(html).toContain('15 分钟')
    const first = state.data!.recentResults[0]
    state.data!.recentResults = Array.from({ length: 10 }, (_, i) => ({
      ...first,
      sessionId: `result-${i}`,
      rawText: `unique-result-${i}`,
    }))
    const limited = render()
    expect(limited).toContain('unique-result-4')
    expect(limited).not.toContain('unique-result-5')
    expect(limited).toContain('查看全部最近结果')
  })
})
