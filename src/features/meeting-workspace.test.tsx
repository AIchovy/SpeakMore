import { describe, expect, it, vi } from 'vitest'
import { renderToStaticMarkup } from 'react-dom/server'
import { PersonalResult, personalItems } from './meeting-workspace'
import type { Meeting, MeetingItem } from './meetings-api'
vi.mock('@/lib/app-state', () => ({ useText: () => (zh: string) => zh }))
const todo = (owner: string | null): MeetingItem => ({
  kind: 'todo',
  text: '更新报价',
  owner,
  due: '周五前',
  evidence: ['s1'],
  done: false,
})
describe('personal results', () => {
  it('matches explicit owners and never assigns missing or similar names to the user', () => {
    expect(personalItems([todo('李明'), todo('李明明'), todo(null)], ' 李明 ')).toEqual([
      todo('李明'),
    ])
    expect(personalItems([todo('李明')], '')).toEqual([])
  })
  it('shows a useful result with source controls and flags incomplete interim results', () => {
    const m = {
      items: [todo('李明'), todo(null), { ...todo(null), kind: 'decision', text: '下周发布' }],
      state: 'queued',
      summaryStale: true,
    } as Meeting
    const html = renderToStaticMarkup(
      <PersonalResult meeting={m} name="李明" onEvidence={() => {}} />,
    )
    expect(html).toContain('我的会后成果')
    expect(html).toContain('更新报价')
    expect(html).toContain('周五前')
    expect(html).toContain('下周发布')
    expect(html).toContain('阶段成果')
    expect(html).toContain('负责人待确认')
  })
  it('learning results contain source facts without inventing personal tasks', () => {
    const m = {
      title: '课程',
      template: 'learning',
      items: [{ ...todo(null), kind: 'fact', text: '预算包含税费' }],
      state: 'ready',
      summaryStale: false,
    } as Meeting
    const html = renderToStaticMarkup(<PersonalResult meeting={m} name="" onEvidence={() => {}} />)
    expect(html).toContain('知识与结论')
    expect(html).toContain('预算包含税费')
    expect(html).toContain('暂无明确的后续事项')
  })
})

describe('trust in personal result cards', () => {
  it('shows only explicit commitments to the user in the owed card', () => {
    const explicit = {
      ...todo('陈雪'),
      recipient: '李明',
      text: '提供验收条件',
      status: 'commitment' as const,
      support: 'supported' as const,
    }
    const unrelated = { ...todo('陈雪'), text: '寄送合同', recipient: '王伟' }
    const m = {
      title: '评审',
      template: 'review',
      items: [explicit, unrelated],
      state: 'ready',
      summaryStale: false,
    } as Meeting
    const html = renderToStaticMarkup(
      <PersonalResult meeting={m} name="李明" onEvidence={() => {}} />,
    )
    expect(html).toContain('谁还欠我什么')
    expect(html).toContain('提供验收条件')
    expect(html).not.toContain('寄送合同')
  })
  it('never presents a proposal or unsupported conclusion as a confirmed decision', () => {
    const proposal = {
      ...todo(null),
      kind: 'next' as const,
      status: 'proposal' as const,
      text: '可以考虑下周上线',
    }
    const unsupported = {
      ...todo(null),
      kind: 'decision' as const,
      status: 'decision' as const,
      support: 'review' as const,
      text: '决定下周五上线',
    }
    const m = {
      title: '评审',
      template: 'review',
      items: [proposal, unsupported],
      state: 'ready',
      summaryStale: false,
    } as Meeting
    const html = renderToStaticMarkup(
      <PersonalResult meeting={m} name="李明" onEvidence={() => {}} />,
    )
    const decisionCard = html.slice(html.indexOf('已经决定了什么'), html.indexOf('我答应了什么'))
    expect(decisionCard).not.toContain(proposal.text)
    expect(decisionCard).not.toContain(unsupported.text)
    expect(html).toContain('哪些事情还没定')
    expect(html).toContain(proposal.text)
  })
})
