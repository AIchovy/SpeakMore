import React, { useEffect, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { invoke, isTauri } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { RecordingBar } from './features/recording-bar'
import type { RecordingStatus } from './lib/types'
import './styles.css'
import './hud.css'

// The status is small; compare it whole because preview and notice changes do
// not advance the session revision.
const statusKey = (status: RecordingStatus) => JSON.stringify(status)

function Hud() {
  const [status, setStatus] = useState<RecordingStatus | null>(null)
  useEffect(() => {
    if (!isTauri()) return
    let disposed = false
    let inFlight = false
    let again = false
    let off: UnlistenFn | undefined
    let retry: ReturnType<typeof setInterval> | undefined
    let shown: string | undefined
    const stopRetry = () => {
      clearInterval(retry)
      retry = undefined
    }
    const refresh = async () => {
      if (inFlight) {
        again = true
        return
      }
      inFlight = true
      do {
        again = false
        try {
          const next = await invoke<RecordingStatus>('get_recording_status')
          if (disposed) break
          const key = statusKey(next)
          if (key !== shown) {
            shown = key
            setStatus(next)
          }
          // Native events deliver every later change once both are available.
          if (off) stopRetry()
        } catch {
          /* A later native event retries initialization. */
        }
      } while (again && !disposed)
      inFlight = false
    }
    // The host can load before native Controls is installed. Retry the initial
    // handshake even when no recording has started to generate another event.
    retry = setInterval(() => {
      void refresh()
    }, 2000)
    void (async () => {
      try {
        off = await listen('recording_changed', () => {
          void refresh()
        })
        if (disposed) off()
      } finally {
        if (!disposed) await refresh()
      }
    })().catch(() => {})
    return () => {
      disposed = true
      stopRetry()
      off?.()
    }
  }, [])
  const available = status !== null
  useEffect(() => {
    document.documentElement.dataset.motion = status?.motionPolicy ?? 'system'
    document.documentElement.lang = status?.uiLocale ?? 'zh-CN'
  }, [status?.motionPolicy, status?.uiLocale])
  useEffect(() => {
    if (available && isTauri()) void invoke('recording_hud_ready').catch(() => {})
  }, [available])
  if (!status) return <div className="recording-bar recording-loading">SpeakMore</div>
  return (
    <RecordingBar
      status={status}
      act={async (cancel) => {
        if (status.notice)
          await invoke('recording_notice_action', {
            sessionId: status.notice.sessionId,
            open: !cancel,
          })
        else if (status.preview) await invoke('preview_recording_panel', { show: false })
        else if (status.session.sessionId)
          await invoke('recording_control', { sessionId: status.session.sessionId, cancel })
      }}
    />
  )
}
createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <Hud />
  </React.StrictMode>,
)
