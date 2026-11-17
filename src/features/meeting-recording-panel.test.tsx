import { describe, expect, it, vi } from 'vitest'
import { renderToStaticMarkup } from 'react-dom/server'
import {
  MeetingRecordingPanel,
  meetingNoteDraftIds,
  meetingNoteDraftSaving,
  readMeetingNoteDraft,
  writeMeetingNoteDraft,
  type MeetingRecordingView,
} from './meeting-recording-panel'

const view: MeetingRecordingView = {
  meetingId: 'meeting',
  title: '项目评审',
  source: 'mixed',
  paused: false,
  stopping: false,
  elapsedMs: 10800000,
  levels: [0.1, 0.2],
  uiLocale: 'zh-CN',
  motionPolicy: 'reduced',
}
const render = (patch: Partial<MeetingRecordingView> = {}, stale = false) =>
  renderToStaticMarkup(
    <MeetingRecordingPanel
      view={{ ...view, ...patch }}
      stale={stale}
      act={async () => {}}
      saveNote={async (_text, _kind, atMs) => atMs}
    />,
  )
describe('meeting floating recording status', () => {
  it('shows long meetings and only the audio sources that are being captured', () => {
    expect(render()).toContain('180:00')
    expect(render()).not.toContain('05:00')
    expect(render({ source: 'system' })).toContain('系统音量')
    expect(render({ source: 'system' })).not.toContain('麦克风音量')
    expect(render({ source: 'microphone' })).not.toContain('系统音量')
    expect(render()).toContain('隐藏悬浮窗，录音继续')
  })
  it('shows a paused meeting with silent meters and resume, rather than a running indicator', () => {
    const html = render({ paused: true })
    expect(html).toContain('会议已暂停')
    expect(html).toContain('继续录音')
    expect(html).not.toContain('会议录音中')
    expect(html.match(/aria-valuenow="0"/g)).toHaveLength(2)
  })
  it('disables recording controls when finishing or status is stale and supports English', () => {
    expect(render({ stopping: true })).toContain('正在保存录音')
    expect(render({}, true)).toContain('状态暂不可用')
    expect(render({}, true).match(/disabled=""/g)).toHaveLength(4)
    const html = render({ uiLocale: 'en', title: 'Project review' })
    expect(html).toContain('Recording meeting')
    expect(html).toContain('Open meeting')
    expect(html).not.toMatch(/[\u4e00-\u9fff]/)
  })
  it('offers a quick note and a moment marker without opening an editor on its own', () => {
    expect(render()).toContain('记一句')
    expect(render()).toContain('标记重点')
    expect(render()).not.toContain('<textarea')
    expect(render({ paused: true })).toMatch(/disabled=""[^>]*title="标记此刻/)
  })
})

class MemoryStorage {
  private values = new Map<string, string>()
  get length() {
    return this.values.size
  }
  key(n: number) {
    return [...this.values.keys()][n] ?? null
  }
  getItem(key: string) {
    return this.values.get(key) ?? null
  }
  setItem(key: string, value: string) {
    this.values.set(key, value)
  }
  removeItem(key: string) {
    this.values.delete(key)
  }
}
describe('floating panel note drafts', () => {
  it('keeps an unsaved note per meeting until it is cleared', () => {
    const storage = new MemoryStorage()
    writeMeetingNoteDraft('a', { text: '确认报价', atMs: 4200 }, storage)
    writeMeetingNoteDraft('b', { text: '   ', atMs: 10 }, storage)
    expect(readMeetingNoteDraft('a', storage)).toEqual({ text: '确认报价', atMs: 4200 })
    expect(readMeetingNoteDraft('b', storage)).toBeNull()
    expect(meetingNoteDraftIds(storage)).toEqual(['a'])
    writeMeetingNoteDraft('a', null, storage)
    expect(meetingNoteDraftIds(storage)).toEqual([])
  })
  it('marks a draft the panel is saving, and treats a mark left behind as unsaved', () => {
    const storage = new MemoryStorage()
    writeMeetingNoteDraft('a', { text: '确认报价', atMs: 10, savingAt: 1000 }, storage)
    const draft = readMeetingNoteDraft('a', storage)!
    expect(draft.savingAt).toBe(1000)
    expect(meetingNoteDraftSaving(draft, 2000)).toBe(true)
    expect(meetingNoteDraftSaving(draft, 31_000)).toBe(false)
    expect(meetingNoteDraftSaving({ text: '确认报价', atMs: 10 }, 2000)).toBe(false)
    writeMeetingNoteDraft('a', { text: '确认报价', atMs: 10 }, storage)
    expect(readMeetingNoteDraft('a', storage)).toEqual({ text: '确认报价', atMs: 10 })
    expect(readMeetingNoteDraft('a', storage)?.savingAt).toBeUndefined()
  })
  it('ignores damaged drafts and works without storage', () => {
    const storage = new MemoryStorage()
    storage.setItem('speakmore.meeting-note-draft:x', '{not json')
    expect(readMeetingNoteDraft('x', storage)).toBeNull()
    expect(readMeetingNoteDraft('x', null)).toBeNull()
    expect(meetingNoteDraftIds(null)).toEqual([])
    expect(() => writeMeetingNoteDraft('x', { text: 'a', atMs: null }, null)).not.toThrow()
  })
  it('restores a draft when the panel is shown again for the same meeting', () => {
    const storage = new MemoryStorage()
    vi.stubGlobal('localStorage', storage)
    try {
      writeMeetingNoteDraft('meeting', { text: '这个报价需要确认', atMs: 1000 })
      expect(render()).toContain('保存便签并结束')
      expect(render()).toContain('继续写便签')
      expect(render({ meetingId: 'other' })).toContain('结束并整理')
    } finally {
      vi.unstubAllGlobals()
    }
  })
})
