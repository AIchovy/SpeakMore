import { meetingMessage } from './meeting-messages'
import { useText } from '@/lib/app-state'
import type { Translate } from '@/lib/errors'
import { useEffect, useState, useSyncExternalStore } from 'react'
import { isTauri } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { Link, useLocation } from 'react-router-dom'
import { toast } from 'sonner'
import { PiMicrophone, PiPause, PiPlay, PiSpeakerHigh, PiStop } from 'react-icons/pi'
import { Button } from '@/components/ui/button'
import { audioLevelFraction } from '@/lib/audio-level'
import { meetingCall, meetingError, timecode, type MeetingList } from './meetings-api'
import {
  meetingNoteDraftIds,
  meetingNoteDraftSaving,
  readMeetingNoteDraft,
  writeMeetingNoteDraft,
} from './meeting-recording-panel'

// Fast polling keeps the recording timer and meters smooth; idle polling only notices new work,
// and meetings_changed (sent when a recording or job starts or ends) wakes it immediately.
const recordingPollMs = 150
const idlePollMs = 2000

// The latest activity, shared so the meeting page shows the same live time as the status bar.
let liveActivity: MeetingList['active'] = null
const activityListeners = new Set<() => void>()
function publishActivity(next: MeetingList['active']) {
  liveActivity = next
  activityListeners.forEach((listener) => listener())
}
function subscribeActivity(listener: () => void) {
  activityListeners.add(listener)
  return () => {
    activityListeners.delete(listener)
  }
}
export function LiveRecordingTime({
  meetingId,
  fallbackMs,
}: {
  meetingId: string
  fallbackMs: number
}) {
  const live = useSyncExternalStore(
    subscribeActivity,
    () =>
      liveActivity?.kind === 'recording' && liveActivity.meetingId === meetingId
        ? Math.floor(liveActivity.elapsedMs / 1000) * 1000
        : null,
    () => null,
  )
  return <>{timecode(live ?? fallbackMs)}</>
}

// Notes typed in the floating panel are kept until the recording ends. When it ends from the main
// window or because the source stopped, the panel cannot save them anymore, so they are added here.
const savingDrafts = new Set<string>()
const failedDrafts = new Set<string>()
// Returns true when a draft was left for later because the panel is still saving it itself.
async function saveNoteDrafts(recordingId: string | null, t: Translate) {
  let deferred = false
  // A resumed meeting gets another chance when its recording ends again.
  if (recordingId) failedDrafts.delete(recordingId)
  for (const id of meetingNoteDraftIds()) {
    if (id === recordingId || savingDrafts.has(id) || failedDrafts.has(id)) continue
    const draft = readMeetingNoteDraft(id)
    if (!draft) {
      writeMeetingNoteDraft(id, null)
      continue
    }
    if (meetingNoteDraftSaving(draft)) {
      deferred = true
      continue
    }
    savingDrafts.add(id)
    try {
      // atMs keeps the linked moment on backends that accept it; others place the note at the end.
      await meetingCall('meeting_note_add', {
        id,
        text: draft.text,
        kind: 'note',
        atMs: draft.atMs,
      })
      if (readMeetingNoteDraft(id)?.text === draft.text) writeMeetingNoteDraft(id, null)
      toast.success(
        t(
          '悬浮窗里未保存的便签已存入会议记录。',
          'An unsaved note from the floating panel was added to the meeting.',
        ),
      )
    } catch {
      failedDrafts.add(id)
      // Keep the draft for the next launch unless its meeting is gone; either way show it once.
      const exists = await meetingCall<MeetingList>('meeting_list')
        .then((list) => list.meetings.some((m) => m.id === id))
        .catch(() => true)
      if (!exists) writeMeetingNoteDraft(id, null)
      toast.error(
        t(
          '悬浮窗里的便签未能存入会议，可复制后手动添加。',
          'A note from the floating panel could not be added to the meeting. Copy it to add it manually.',
        ),
        {
          description: draft.text,
          duration: 60000,
          action: {
            label: t('复制便签', 'Copy note'),
            onClick: () => void meetingCall('captions_copy', { text: draft.text }).catch(() => {}),
          },
        },
      )
    } finally {
      savingDrafts.delete(id)
    }
  }
  return deferred
}

export function MeetingActivity() {
  const t = useText()
  const { pathname } = useLocation()
  const [a, setA] = useState<MeetingList['active']>(null)
  useEffect(() => {
    if (!isTauri()) return
    let live = true,
      reading = false,
      again = false,
      timer: ReturnType<typeof setTimeout> | undefined,
      // Recording meeting when drafts were last checked; undefined checks on the first read.
      draftsCheckedFor: string | null | undefined
    const offs: (() => void)[] = []
    const read = () => {
      if (!live) return
      if (reading) {
        again = true
        return
      }
      reading = true
      clearTimeout(timer)
      let recording = false
      void meetingCall<MeetingList['active']>('meeting_activity')
        .then((next) => {
          if (!live) return
          setA(next)
          publishActivity(next)
          recording = next?.kind === 'recording'
          const id = recording ? next!.meetingId : null
          if (id !== draftsCheckedFor) {
            draftsCheckedFor = id
            void saveNoteDrafts(id, t).then((deferred) => {
              // Check again on a later read, once the panel has saved the note or given up.
              if (deferred && draftsCheckedFor === id) draftsCheckedFor = undefined
            })
          }
        })
        .catch(() => {})
        .finally(() => {
          reading = false
          if (!live) return
          if (again) {
            again = false
            read()
          } else timer = setTimeout(read, recording ? recordingPollMs : idlePollMs)
        })
    }
    read()
    const subscribe = (promise: Promise<() => void>) =>
      void promise.then((off) => {
        if (live) offs.push(off)
        else off()
      })
    subscribe(listen('meetings_changed', read))
    subscribe(
      listen<string>('meeting_notice', (e) =>
        toast.info(meetingMessage(e.payload, t), { duration: 15000 }),
      ),
    )
    return () => {
      live = false
      clearTimeout(timer)
      offs.forEach((off) => off())
    }
  }, [t])
  if (a?.kind !== 'recording' || !a.meetingId) return null
  const control = (action: string) =>
    void meetingCall('meeting_control', { id: a.meetingId, action }).catch((e) =>
      toast.error(meetingMessage(meetingError(e), t)),
    )
  return (
    <MeetingRecordingBar activity={a} showControls={pathname !== '/meetings'} onControl={control} />
  )
}

export function MeetingRecordingBar({
  activity: a,
  showControls,
  onControl,
}: {
  activity: NonNullable<MeetingList['active']>
  showControls: boolean
  onControl: (action: 'pause' | 'resume' | 'stop') => void
}) {
  const t = useText()

  return (
    <div
      aria-label={t('会议录音状态', 'Meeting recording status')}
      className="mb-6 flex flex-wrap items-center justify-between gap-x-6 gap-y-4 rounded-xl border border-border bg-muted/50 px-5 py-4"
    >
      <Link
        to="/meetings"
        className="flex items-center gap-4 rounded-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
      >
        <span className="flex items-center gap-2 text-sm font-medium" role="status">
          {a.paused ? (
            <PiPause aria-hidden className="size-4" />
          ) : (
            <span aria-hidden className="size-2 rounded-full bg-foreground" />
          )}
          {a.paused ? t('会议已暂停', 'Meeting paused') : t('会议录音中', 'Recording meeting')}
        </span>
        <span className="flex items-baseline gap-1.5 tabular-nums">
          <span className="text-lg font-medium tracking-tight">{timecode(a.elapsedMs)}</span>
        </span>
      </Link>
      <div className="flex flex-wrap items-center gap-x-6 gap-y-3">
        <Button
          variant="ghost"
          size="sm"
          onClick={() =>
            void meetingCall('meeting_recording_window', { id: a.meetingId, action: 'show' }).catch(
              (e) => toast.error(meetingMessage(meetingError(e), t)),
            )
          }
        >
          {t('悬浮状态', 'Show floating status')}
        </Button>
        <div className="flex items-center gap-4">
          {a.levels?.map((v, i) => {
            const level = a.paused ? 0 : audioLevelFraction(v)
            const Icon = i ? PiSpeakerHigh : PiMicrophone
            return (
              <div key={i} className="flex items-center gap-2 text-muted-foreground">
                <Icon aria-hidden className="size-4" />
                <div
                  role="meter"
                  aria-label={
                    i ? t('系统音量', 'System audio level') : t('麦克风音量', 'Microphone level')
                  }
                  aria-valuemin={0}
                  aria-valuemax={100}
                  aria-valuenow={Math.round(level * 100)}
                  className="flex h-4 items-center gap-0.5"
                >
                  {Array.from({ length: 8 }, (_, n) => (
                    <span
                      key={n}
                      className={`h-3 w-0.5 rounded-full ${level > n / 8 ? 'bg-foreground/70' : 'bg-foreground/15'}`}
                    />
                  ))}
                </div>
              </div>
            )
          })}
        </div>
        {showControls && (
          <div className="flex items-center gap-2">
            <Button variant="outline" onClick={() => onControl(a.paused ? 'resume' : 'pause')}>
              {a.paused ? <PiPlay /> : <PiPause />}
              {a.paused ? t('继续录音', 'Resume recording') : t('暂停', 'Pause')}
            </Button>
            <Button onClick={() => onControl('stop')}>
              <PiStop />
              {t('结束并整理', 'Finish and summarize')}
            </Button>
          </div>
        )}
      </div>
    </div>
  )
}
