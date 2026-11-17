import React, { useEffect, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { isTauri } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import {
  MeetingRecordingPanel,
  type MeetingRecordingView,
  type MeetingPanelAction,
  type MeetingNoteKind,
} from './features/meeting-recording-panel'
import { meetingCall } from './features/meetings-api'
import './styles.css'
import './meeting-hud.css'

function MeetingHud() {
  const [view, setView] = useState<MeetingRecordingView | null>(null)
  const [loaded, setLoaded] = useState(false)
  const [stale, setStale] = useState(false)
  useEffect(() => {
    let live = true,
      reading = false
    let off: (() => void) | undefined
    const refresh = async () => {
      if (reading || !live) return
      reading = true
      try {
        const next = await meetingCall<MeetingRecordingView | null>('meeting_recording_view')
        if (live) {
          setView(next)
          setLoaded(true)
          setStale(false)
        }
      } catch {
        if (live) setStale(true)
      } finally {
        reading = false
      }
    }
    void refresh()
    const timer = setInterval(() => void refresh(), 250)
    if (isTauri())
      void listen('meeting_recording_changed', () => void refresh()).then((f) => {
        if (live) off = f
        else f()
      })
    else window.addEventListener('meeting_hud_preview_changed', refresh)
    return () => {
      live = false
      clearInterval(timer)
      off?.()
      window.removeEventListener('meeting_hud_preview_changed', refresh)
    }
  }, [])
  useEffect(() => {
    if (loaded && isTauri())
      void meetingCall('meeting_recording_window', { action: 'ready', id: null }).catch(() => {})
  }, [loaded])
  useEffect(() => {
    document.documentElement.lang = view?.uiLocale ?? 'zh-CN'
    document.documentElement.dataset.motion = view?.motionPolicy ?? 'system'
  }, [view?.uiLocale, view?.motionPolicy])
  if (!view)
    return <div className="meeting-panel meeting-panel-empty">{loaded ? 'SpeakMore' : '…'}</div>
  const act = async (action: MeetingPanelAction) => {
    await meetingCall(
      ['pause', 'resume', 'stop'].includes(action)
        ? 'meeting_recording_control'
        : 'meeting_recording_window',
      { id: view.meetingId, action },
    )
    const next = await meetingCall<MeetingRecordingView | null>('meeting_recording_view')
    setView(next)
  }
  const saveNote = async (text: string, kind: MeetingNoteKind, atMs: number) =>
    meetingCall<number>('meeting_recording_note', { id: view.meetingId, text, kind, atMs })
  return (
    <MeetingRecordingPanel
      key={view.meetingId}
      view={view}
      stale={stale}
      act={act}
      saveNote={saveNote}
    />
  )
}
createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <MeetingHud />
  </React.StrictMode>,
)
