import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderToStaticMarkup } from 'react-dom/server'
import { MemoryRouter } from 'react-router-dom'
import { createPreview } from '@/lib/preview'
import type { Snapshot } from '@/lib/types'
import { Meetings } from './meetings'
import { Captions } from './captions'
import { MeetingModel } from './meeting-model'
import { MeetingRecordingBar } from './meeting-activity'
import { ConversationAssistant } from './conversation-assistant'
import { meetingStatus, meetingDownloadLabel, meetingStatusLabel } from './meetings-api'
import { meetingMessage } from './meeting-messages'
import languages from '../../resources/caption-languages.json'
import { MeetingMicrophoneAccess } from './meeting-microphone-access'

const state = vi.hoisted(() => ({ english: false, data: null as Snapshot | null }))
vi.mock('@/lib/app-state', () => ({
  useText: () => (zh: string, en: string) => (state.english ? en : zh),
  useApp: () => ({ data: state.data }),
  call: vi.fn(),
}))
const render = (node: React.ReactNode) => renderToStaticMarkup(<MemoryRouter>{node}</MemoryRouter>)
const t = (zh: string, en: string) => (state.english ? en : zh)
beforeEach(async () => {
  state.data = (await createPreview()('get_snapshot')) as Snapshot
  state.english = false
})

describe('meeting and caption interface language', () => {
  it('distinguishes legacy recordings that never started from failures with saved audio', () => {
    const empty = {
      state: 'failed',
      source: 'microphone',
      durationMs: 0,
      processedSamples: 0,
      segments: [],
    }
    expect(meetingStatusLabel(empty, t)).toBe('未开始录音')
    expect(meetingStatusLabel({ ...empty, durationMs: 5000 }, t)).toBe('需要处理')
    expect(meetingStatusLabel({ ...empty, source: 'import' }, t)).toBe('需要处理')
    state.english = true
    expect(meetingStatusLabel(empty, t)).toBe('Recording did not start')
  })
  it('offers microphone authorization before capture and settings after a denial', () => {
    state.data!.desktop = true
    const panel = () =>
      render(
        <MeetingMicrophoneAccess
          source="microphone"
          denied={false}
          disabled={false}
          onRecheck={() => {}}
        />,
      )
    state.data!.permissions.microphone = 'not_determined'
    expect(panel()).toContain('授权麦克风')
    expect(panel()).toContain('授权后请再次点击')
    state.data!.permissions.microphone = 'denied'
    expect(panel()).toContain('打开系统设置')
    expect(panel()).toContain('重新检查权限')
    state.english = true
    expect(panel()).toContain('Open System Settings')
    expect(panel()).not.toMatch(/[\u4e00-\u9fff]/)
  })
  it('does not require a microphone for system audio or block an authorized source', () => {
    state.data!.desktop = true
    state.data!.permissions.microphone = 'denied'
    expect(
      render(
        <MeetingMicrophoneAccess
          source="system"
          denied={true}
          disabled={false}
          onRecheck={() => {}}
        />,
      ),
    ).toBe('')
    state.data!.permissions.microphone = 'authorized'
    expect(
      render(
        <MeetingMicrophoneAccess
          source="mixed"
          denied={false}
          disabled={false}
          onRecheck={() => {}}
        />,
      ),
    ).toBe('')
    expect(
      render(
        <MeetingMicrophoneAccess
          source="mixed"
          denied={true}
          disabled={false}
          onRecheck={() => {}}
        />,
      ),
    ).toContain('打开系统设置')
  })
  it('shows a recording beyond one hour without a duration cap in either language', () => {
    for (const english of [false, true]) {
      state.english = english
      const html = render(
        <MeetingRecordingBar
          activity={{
            id: 'a',
            meetingId: 'm',
            kind: 'recording',
            paused: false,
            elapsedMs: 10800000,
            levels: [0, 0],
          }}
          showControls
          onControl={() => {}}
        />,
      )
      expect(html).toContain('180:00')
      expect(html).not.toContain('/ 60:00')
      expect(html).toContain(english ? 'Finish and summarize' : '结束并整理')
    }
  })
  it('renders English across meetings, captions, models, recording status and assistant', () => {
    state.english = true
    for (const [node, expected] of [
      [<Meetings />, 'Start meeting'],
      [<Captions />, 'Both can be enabled together'],
      [<MeetingModel />, 'Local text model'],
      [
        <ConversationAssistant meetingId="meeting" contextKey="test" />,
        'Keep the conversation going.',
      ],
      [<ConversationAssistant surface="captions" contextKey="captions" />, 'Explore this content.'],
      [
        <MeetingRecordingBar
          activity={{
            id: 'a',
            meetingId: 'm',
            kind: 'recording',
            paused: true,
            elapsedMs: 0,
            levels: [0, 0],
          }}
          showControls
          onControl={() => {}}
        />,
        'Meeting paused',
      ],
    ] as const) {
      const html = render(node)
      expect(html).toContain(expected)
      expect(html).not.toMatch(/[\u4e00-\u9fff]/)
    }
  })
  it('switches labels back to Chinese without changing language codes', () => {
    expect(render(<Captions />)).toContain('两项可同时开启')
    expect(meetingStatus(t).paused).toBe('已暂停')
    state.english = true
    expect(meetingStatus(t).paused).toBe('Paused')
    expect(render(<Captions />)).toContain('Translate to')
    expect(languages.find((l) => l.code === 'zh-CN')?.englishName).toBe('Chinese (Simplified)')
    expect(languages.every((l) => l.name && l.englishName)).toBe(true)
    state.english = false
    expect(render(<Meetings />)).toContain('开始会议')
  })
  it('localizes persisted operational messages and download phases at render time', () => {
    const message = '字幕已为听写暂停，听写结束后自动恢复。'
    expect(meetingMessage(message, t)).toBe(message)
    state.english = true
    expect(meetingMessage(message, t)).toContain('paused for dictation')
    expect(
      meetingDownloadLabel(
        { downloadPhase: 'installing_runtime' } as Parameters<typeof meetingDownloadLabel>[0],
        t,
      ),
    ).toBe('Installing and checking the MOSS runtime')
    expect(meetingMessage('字幕 · 麦克风', t)).toBe('Captions · Microphone')
    expect(meetingMessage('A custom diagnostic', t)).toBe('A custom diagnostic')
  })
})
