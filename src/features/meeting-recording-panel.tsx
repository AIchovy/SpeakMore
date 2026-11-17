import { useEffect, useRef, useState } from 'react'
import {
  PiArrowSquareOut,
  PiMicrophone,
  PiPause,
  PiPlay,
  PiSpeakerHigh,
  PiStop,
  PiX,
  PiNotePencil,
  PiBookmarkSimple,
} from 'react-icons/pi'
import { audioLevelFraction } from '@/lib/audio-level'
import { meetingError, timecode } from './meetings-api'
import { meetingMessage } from './meeting-messages'

export interface MeetingRecordingView {
  meetingId: string
  title: string
  source: string
  paused: boolean
  stopping: boolean
  elapsedMs: number
  levels: number[]
  uiLocale: string
  motionPolicy: string
}
export type MeetingPanelAction =
  'pause' | 'resume' | 'stop' | 'open' | 'hide' | 'drag' | 'compose' | 'dismiss_note'
export type MeetingNoteKind = 'note' | 'highlight'

// A note being written outlives the floating panel: it is kept per meeting in storage shared with
// the main window, which adds it to the meeting when the recording ends elsewhere.
export interface MeetingNoteDraft {
  text: string
  atMs: number | null
  /** Set while the panel itself is saving the note, so the main window does not add it too. */
  savingAt?: number
}
type DraftStorage = Pick<Storage, 'getItem' | 'setItem' | 'removeItem' | 'key' | 'length'>
const draftPrefix = 'speakmore.meeting-note-draft:'
// A panel closed mid-save leaves the mark behind; after this the draft counts as unsaved again.
const draftSavingMs = 30_000
export function meetingNoteDraftSaving(draft: MeetingNoteDraft, now = Date.now()) {
  const age = draft.savingAt === undefined ? -1 : now - draft.savingAt
  return age >= 0 && age < draftSavingMs
}
function browserStorage(): DraftStorage | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage
  } catch {
    return null
  }
}
export function readMeetingNoteDraft(
  meetingId: string,
  storage = browserStorage(),
): MeetingNoteDraft | null {
  try {
    const value = JSON.parse(
      storage?.getItem(draftPrefix + meetingId) ?? 'null',
    ) as Partial<MeetingNoteDraft> | null
    if (typeof value?.text !== 'string' || !value.text.trim()) return null
    return {
      text: value.text.slice(0, 1200),
      atMs: typeof value.atMs === 'number' && value.atMs >= 0 ? value.atMs : null,
      ...(typeof value.savingAt === 'number' && { savingAt: value.savingAt }),
    }
  } catch {
    return null
  }
}
export function writeMeetingNoteDraft(
  meetingId: string,
  draft: MeetingNoteDraft | null,
  storage = browserStorage(),
) {
  try {
    if (draft?.text.trim()) storage?.setItem(draftPrefix + meetingId, JSON.stringify(draft))
    else storage?.removeItem(draftPrefix + meetingId)
  } catch {
    // Unavailable storage: the draft stays in the open panel only.
  }
}
export function meetingNoteDraftIds(storage = browserStorage()) {
  const ids: string[] = []
  try {
    for (let n = 0; storage && n < storage.length; n++) {
      const key = storage.key(n)
      if (key?.startsWith(draftPrefix)) ids.push(key.slice(draftPrefix.length))
    }
  } catch {
    // Unavailable storage has no drafts.
  }
  return ids
}

export function MeetingRecordingPanel({
  view,
  stale = false,
  act,
  saveNote,
}: {
  view: MeetingRecordingView
  stale?: boolean
  act: (action: MeetingPanelAction) => Promise<void>
  saveNote: (text: string, kind: MeetingNoteKind, atMs: number) => Promise<number>
}) {
  const t = (zh: string, en: string) => (view.uiLocale === 'en' ? en : zh)
  const [pending, setPending] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [editing, setEditing] = useState(false)
  const [restored] = useState(() => readMeetingNoteDraft(view.meetingId))
  const [note, setNote] = useState(restored?.text ?? '')
  const [noteAt, setNoteAt] = useState<number | null>(restored?.atMs ?? null)
  const [feedback, setFeedback] = useState('')
  const working = useRef(false)
  const input = useRef<HTMLTextAreaElement>(null)
  useEffect(() => {
    if (editing) {
      const frame = requestAnimationFrame(() => input.current?.focus())
      return () => cancelAnimationFrame(frame)
    }
  }, [editing])
  // The main window adds a stored draft once the recording ends elsewhere. While the note is
  // saved here the draft is marked, so it is not added twice; it is cleared once saved, and a
  // failed save keeps it as a draft unless the main window has taken it over meanwhile.
  const saveDraftNote = async (atMs: number) => {
    const text = note,
      savingAt = Date.now()
    writeMeetingNoteDraft(view.meetingId, { text, atMs, savingAt })
    try {
      const savedAt = await saveNote(text.trim(), 'note', atMs)
      writeMeetingNoteDraft(view.meetingId, null)
      return savedAt
    } catch (failure) {
      if (readMeetingNoteDraft(view.meetingId)?.savingAt === savingAt)
        writeMeetingNoteDraft(view.meetingId, { text, atMs: noteAt })
      throw failure
    }
  }
  const perform = async (action: MeetingPanelAction) => {
    if (working.current) return false
    working.current = true
    setPending(true)
    setError(null)
    try {
      if (action === 'stop' && note.trim()) {
        // Saved and cleared before stopping so the main window does not add the same note again.
        const savedAt = await saveDraftNote(noteAt ?? view.elapsedMs)
        setNote('')
        setNoteAt(null)
        setFeedback(t(`${timecode(savedAt)} 便签已保存`, `Note saved at ${timecode(savedAt)}`))
      }
      await act(action)
      if (action === 'hide') setEditing(false)
      return true
    } catch (failure) {
      setError(meetingError(failure))
      return false
    } finally {
      setPending(false)
      working.current = false
    }
  }
  const toggleEditor = async (open: boolean) => {
    const atMs = noteAt ?? view.elapsedMs
    if (await perform(open ? 'compose' : 'dismiss_note')) {
      if (open) setNoteAt(atMs)
      else if (!note.trim()) setNoteAt(null)
      setEditing(open)
    }
  }
  const save = async (kind: MeetingNoteKind) => {
    if (
      working.current ||
      disabled ||
      (kind === 'note' && !note.trim()) ||
      (kind === 'highlight' && view.paused)
    )
      return
    working.current = true
    setPending(true)
    setError(null)
    const atMs = kind === 'highlight' ? view.elapsedMs : (noteAt ?? view.elapsedMs)
    try {
      const savedAt =
        kind === 'highlight' ? await saveNote('', kind, atMs) : await saveDraftNote(atMs)
      setFeedback(
        kind === 'highlight'
          ? t(`${timecode(savedAt)} 已标记重点`, `Highlight saved at ${timecode(savedAt)}`)
          : t(`${timecode(savedAt)} 便签已保存`, `Note saved at ${timecode(savedAt)}`),
      )
      if (kind === 'note') {
        setNote('')
        setNoteAt(null)
        try {
          await act('dismiss_note')
          setEditing(false)
        } catch {
          setError(
            t('便签已保存，窗口未收起，可手动收起。', 'Note saved. Close the editor manually.'),
          )
        }
      }
    } catch (failure) {
      setError(meetingError(failure))
    } finally {
      setPending(false)
      working.current = false
    }
  }
  const disabled = pending || view.stopping || stale
  const label = stale
    ? t('正在确认录音状态', 'Checking recording status')
    : view.stopping
      ? t('正在保存录音', 'Saving recording')
      : view.paused
        ? t('会议已暂停', 'Meeting paused')
        : t('会议录音中', 'Recording meeting')
  return (
    <section
      className="meeting-panel"
      aria-label={t('会议悬浮录音状态', 'Floating meeting recording status')}
      data-paused={view.paused}
      data-editing={editing}
    >
      <div className="meeting-panel-heading">
        <div
          className="meeting-panel-drag"
          title={t('拖动悬浮窗', 'Drag recording panel')}
          onPointerDown={(event) => {
            if (event.button === 0) void act('drag').catch(() => {})
          }}
        >
          <span
            className="meeting-panel-dot"
            data-idle={view.paused || stale || view.stopping}
            aria-hidden
          />
          <span role="status">{label}</span>
        </div>
        <span className="meeting-panel-time">{timecode(view.elapsedMs)}</span>
        <button
          className="meeting-panel-hide"
          title={t('隐藏悬浮窗，录音继续', 'Hide panel; recording continues')}
          aria-label={t('隐藏悬浮窗，录音继续', 'Hide panel; recording continues')}
          disabled={pending}
          onClick={() => void perform('hide')}
        >
          <PiX />
        </button>
      </div>
      <div className="meeting-panel-context">
        <span className="meeting-panel-title" title={view.title}>
          {view.title}
        </span>
        <div className="meeting-panel-levels">
          {(
            [
              ['microphone', PiMicrophone],
              ['system', PiSpeakerHigh],
            ] as const
          )
            .filter(([source]) => view.source === source || view.source === 'mixed')
            .map(([source, Icon]) => {
              const channel = source === 'microphone' ? 0 : 1
              const level =
                view.paused || stale || view.stopping
                  ? 0
                  : audioLevelFraction(view.levels[channel] ?? 0)
              return (
                <span key={source} className="meeting-panel-source">
                  <Icon aria-hidden />
                  <span
                    role="meter"
                    aria-label={
                      source === 'microphone'
                        ? t('麦克风音量', 'Microphone level')
                        : t('系统音量', 'System audio level')
                    }
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-valuenow={Math.round(level * 100)}
                    className="meeting-panel-meter"
                  >
                    {Array.from({ length: 6 }, (_, n) => (
                      <i key={n} data-filled={level > n / 6} />
                    ))}
                  </span>
                </span>
              )
            })}
        </div>
      </div>
      <div className="meeting-panel-controls">
        <button disabled={disabled} onClick={() => void perform(view.paused ? 'resume' : 'pause')}>
          {view.paused ? <PiPlay /> : <PiPause />}
          {view.paused ? t('继续录音', 'Resume') : t('暂停', 'Pause')}
        </button>
        <button
          className="meeting-panel-finish"
          disabled={disabled}
          onClick={() => void perform('stop')}
        >
          <PiStop />
          {note.trim()
            ? t('保存便签并结束', 'Save note & finish')
            : t('结束并整理', 'Finish & summarize')}
        </button>
        <button
          className="meeting-panel-open"
          disabled={pending}
          onClick={() => void perform('open')}
        >
          <PiArrowSquareOut />
          {t('打开会议', 'Open meeting')}
        </button>
      </div>
      <div className="meeting-panel-note-actions">
        <button
          disabled={disabled || view.paused}
          onClick={() => void save('highlight')}
          title={t('标记此刻，整理纪要时优先参考附近讨论', 'Mark this moment for the summary')}
        >
          <PiBookmarkSimple />
          {t('标记重点', 'Mark highlight')}
        </button>
        <button disabled={disabled} onClick={() => void toggleEditor(!editing)}>
          <PiNotePencil />
          {editing
            ? t('收起便签', 'Close note')
            : note
              ? t('继续写便签', 'Continue note')
              : t('记一句', 'Quick note')}
        </button>
      </div>
      {editing && (
        <form
          className="meeting-panel-editor"
          onSubmit={(event) => {
            event.preventDefault()
            void save('note')
          }}
        >
          <textarea
            ref={input}
            aria-label={t('会议便签', 'Meeting note')}
            placeholder={t('例如：这个报价需要确认', 'For example: Confirm this quote')}
            maxLength={1200}
            value={note}
            disabled={pending || view.stopping}
            onChange={(event) => {
              setNote(event.target.value)
              writeMeetingNoteDraft(view.meetingId, { text: event.target.value, atMs: noteAt })
            }}
            onKeyDown={(event) => {
              if (event.nativeEvent.isComposing) return
              if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') {
                event.preventDefault()
                void save('note')
              } else if (event.key === 'Escape') {
                event.preventDefault()
                void toggleEditor(false)
              }
            }}
          />
          <div className="meeting-panel-note-meta">
            <span>
              {t('关联讨论：', 'Linked at: ')}
              {timecode(noteAt ?? view.elapsedMs)}
            </span>
            <span>{note.length}/1200</span>
          </div>
          <div className="meeting-panel-note-submit">
            <span>{t('⌘/Ctrl + Enter 保存', '⌘/Ctrl + Enter to save')}</span>
            <button type="submit" disabled={disabled || !note.trim()}>
              {pending ? t('保存中…', 'Saving…') : t('保存便签', 'Save note')}
            </button>
          </div>
        </form>
      )}
      <p
        className="meeting-panel-note"
        role={error ? 'alert' : 'status'}
        title={error ? meetingMessage(error, t) : undefined}
      >
        {error
          ? meetingMessage(error, t)
          : stale
            ? t('状态暂不可用，请打开会议检查。', 'Status unavailable. Open the meeting to check.')
            : feedback
              ? feedback
              : view.paused
                ? t(
                    '暂停期间不收音；点击继续恢复录音。',
                    'Audio capture is paused. Resume when ready.',
                  )
                : t(
                    '录音保存在本机，切换应用仍会继续。',
                    'Saved on this device. Recording continues across apps.',
                  )}
      </p>
    </section>
  )
}
