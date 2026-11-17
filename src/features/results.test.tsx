import { describe, expect, it, vi } from 'vitest'
import { renderToStaticMarkup } from 'react-dom/server'
import type { ResultRecord } from '@/lib/types'
import { defaultVariant, ResultBody, refinementReason, refinementStatus } from './results'
vi.mock('@/lib/app-state', () => ({ useText: () => (zh: string) => zh, call: vi.fn() }))
const result = {
  sessionId: 'fixture',
  rawText: '<script>no</script>原文',
  polishedText: '整理后',
  polishOutcome: 'applied',
  deliveryStatus: 'uncertain',
  deliveryReason: null,
  needsReview: true,
} as ResultRecord
describe('result detail', () => {
  it('distinguishes successful unchanged text from disabled or failed refinement', () => {
    const en = (_: string, value: string) => value
    expect(refinementStatus({ ...result, polishedText: result.rawText }, en)).toContain(
      'no changes needed',
    )
    expect(refinementStatus({ ...result, polishOutcome: 'off', polishedText: null }, en)).toContain(
      'was off',
    )
    expect(
      refinementStatus({ ...result, polishOutcome: 'fallback', polishReason: 'deadline' }, en),
    ).toContain('2.5 seconds')
    expect(
      refinementStatus(
        { ...result, polishOutcome: 'fallback', polishReason: 'private upstream data' },
        en,
      ),
    ).not.toContain('private upstream data')
  })
  it('shows actionable refinement reasons and never renders unknown provider content', () => {
    const html = renderToStaticMarkup(
      <ResultBody
        record={{ ...result, polishOutcome: 'skipped', polishReason: 'text_too_long' }}
      />,
    )
    expect(html).toContain('4,000 字符')
    expect(refinementReason('private upstream response', (_, en) => en)).toBeNull()
    expect(refinementReason('deadline', (_, en) => en)).toContain('2.5 seconds')
  })
  it('explains manual fallback from a known code without rendering unknown target data', () => {
    const record = { ...result, deliveryStatus: 'ready_for_copy', deliveryReason: 'target_changed' }
    expect(renderToStaticMarkup(<ResultBody record={record} />)).toContain('焦点、选区或文字已变化')
    record.deliveryReason = 'private target fragment'
    expect(renderToStaticMarkup(<ResultBody record={record} />)).not.toContain(
      'private target fragment',
    )
  })
  it('defaults to the actual applied version and preserves fallback original', () => {
    expect(defaultVariant(result)).toBe('polished')
    const fallback = { ...result, polishOutcome: 'fallback' }
    expect(defaultVariant(fallback)).toBe('raw')
    const html = renderToStaticMarkup(<ResultBody record={fallback} />)
    expect(html).toContain('整理未完成，已保留原文')
    expect(html).toContain('请检查目标应用')
    expect(html).toContain('&lt;script&gt;no&lt;/script&gt;')
    expect(html).not.toContain('<script>')
    expect(html).toContain('请检查后使用')
  })
})
