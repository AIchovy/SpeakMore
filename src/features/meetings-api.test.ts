import { describe, expect, it } from 'vitest'
import {
  meetingChangedMessage,
  meetingEditError,
  meetingKey,
  meetingListKey,
  meetingMissingMessage,
  type Meeting,
  type MeetingList,
} from './meetings-api'
import { meetingMessage } from './meeting-messages'

const list = (patch: Partial<MeetingList> = {}): MeetingList => ({
  meetings: [{ id: 'm1', revision: 3 } as Meeting],
  active: {
    id: 'a',
    meetingId: 'm1',
    kind: 'recording',
    paused: false,
    elapsedMs: 1000,
    levels: [0.1, 0.2],
  },
  textModelReady: false,
  textModelBytes: 10,
  downloadedBytes: 0,
  downloadError: null,
  downloadModel: null,
  ...patch,
})

describe('meeting polling identities', () => {
  it('ignores the live timer and meters but notices content and state changes', () => {
    const key = meetingListKey(list())
    expect(
      meetingListKey(list({ active: { ...list().active!, elapsedMs: 9000, levels: [1, 1] } })),
    ).toBe(key)
    expect(meetingListKey(list({ active: { ...list().active!, paused: true } }))).not.toBe(key)
    expect(meetingListKey(list({ meetings: [{ id: 'm1', revision: 4 } as Meeting] }))).not.toBe(key)
    expect(meetingListKey(list({ downloadedBytes: 5 }))).not.toBe(key)
    expect(meetingListKey(list({ active: null }))).not.toBe(key)
  })
  it('treats result cards that became stale as a change without a new revision', () => {
    const meeting = { id: 'm1', revision: 3, artifacts: [{ stale: false }] } as Meeting
    expect(meetingKey({ ...meeting })).toBe(meetingKey(meeting))
    expect(meetingKey({ ...meeting, artifacts: [{ stale: true }] } as Meeting)).not.toBe(
      meetingKey(meeting),
    )
    expect(meetingKey({ ...meeting, revision: 4 })).not.toBe(meetingKey(meeting))
  })
})

describe('meeting edit errors', () => {
  it('explains a revision conflict as a meeting change, in both languages', () => {
    const conflict = {
      code: 'RevisionConflict',
      userMessage: '设置已在其他位置改变，请刷新后重试。',
    }
    expect(meetingEditError(conflict)).toBe(meetingChangedMessage)
    expect(meetingEditError({ code: 'InvalidInput', userMessage: '原文格式无效。' })).toBe(
      '原文格式无效。',
    )
    const english = (_zh: string, en: string) => en
    for (const message of [meetingChangedMessage, meetingMissingMessage])
      expect(meetingMessage(message, english)).not.toMatch(/[一-鿿]/)
  })
})
