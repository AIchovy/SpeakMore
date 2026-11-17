import { describe, expect, it, vi } from 'vitest'
import { renderToStaticMarkup } from 'react-dom/server'
import { ItemState } from './meeting-polish'
import { reviewSources } from './meeting-item-review'
import { meetingStatus, type Meeting, type MeetingItem } from './meetings-api'

vi.mock('@/lib/app-state', () => ({ useText: () => (zh: string) => zh }))
const item: MeetingItem = {
  kind: 'todo',
  text: '发送报价',
  owner: null,
  due: null,
  evidence: ['s2', 's2', 'missing'],
  done: false,
}
describe('meeting evidence review', () => {
  it('distinguishes AI support from user confirmation and manual editing', () => {
    const render = (patch: Partial<MeetingItem>) =>
      renderToStaticMarkup(<ItemState item={{ ...item, ...patch }} />)
    expect(render({ support: 'supported' })).toContain('有原文支持')
    expect(render({ support: 'supported' })).not.toContain('你已确认')
    expect(render({ support: 'supported', reviewed: true })).toContain('你已确认与原文一致')
    expect(render({ support: 'supported', reviewed: true })).not.toContain('已手动编辑')
    expect(render({ support: 'review', edited: true })).toContain('需核对原文')
    expect(render({ support: 'review', edited: true })).toContain('已手动编辑')
  })
  it('shows only actual cited excerpts without duplicate or invented sources', () => {
    const meeting = {
      items: [item],
      segments: [
        { id: 's1', text: '背景' },
        { id: 's2', text: '我来发送报价' },
      ],
    } as Meeting
    expect(reviewSources(meeting, 0).map((s) => s.id)).toEqual(['s2'])
    expect(reviewSources({ ...meeting, segments: [] }, 0)).toEqual([])
  })
  it('does not describe completed processing as a required review step', () => {
    expect(meetingStatus((zh) => zh).ready).toBe('纪要已生成')
  })
})
