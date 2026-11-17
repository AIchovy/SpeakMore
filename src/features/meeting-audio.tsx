import { useEffect, useRef, useState } from 'react'
import { PiPlay } from 'react-icons/pi'
import { useText } from '@/lib/app-state'
import { Button } from '@/components/ui/button'
import { meetingCall, meetingError } from './meetings-api'
import { meetingMessage } from './meeting-messages'

export function MeetingAudio({
  meetingId,
  startMs,
  deleted,
}: {
  meetingId: string
  startMs: number
  deleted: boolean
}) {
  const t = useText()
  const [url, setUrl] = useState<string | null>(null)
  const [pending, setPending] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [playing, setPlaying] = useState(false)
  const generation = useRef(0)
  const currentUrl = useRef<string | null>(null)
  useEffect(
    () => () => {
      ++generation.current
      if (currentUrl.current) URL.revokeObjectURL(currentUrl.current)
      currentUrl.current = null
    },
    [],
  )

  const load = async () => {
    const request = ++generation.current
    setPending(true)
    setError(null)
    setPlaying(false)
    setUrl(null)
    if (currentUrl.current) URL.revokeObjectURL(currentUrl.current)
    currentUrl.current = null
    try {
      const bytes = await meetingCall<number[]>('meeting_audio', { id: meetingId, startMs })
      if (request !== generation.current) return
      if (bytes.length <= 44) throw new Error('此片段没有可回听的录音，仍可查看逐字稿。')
      const next = URL.createObjectURL(new Blob([new Uint8Array(bytes)], { type: 'audio/wav' }))
      currentUrl.current = next
      setUrl(next)
    } catch (failure) {
      if (request === generation.current) setError(meetingError(failure))
    } finally {
      if (request === generation.current) setPending(false)
    }
  }
  if (deleted)
    return (
      <p role="status" className="text-sm text-muted-foreground">
        {t(
          '录音已删除，仍可查看逐字稿。',
          'The recording was deleted. The transcript is still available.',
        )}
      </p>
    )
  return (
    <div className="space-y-3">
      {url && (
        <>
          <audio
            controls
            autoPlay
            preload="auto"
            src={url}
            className="w-full"
            aria-label={t('会议片段播放器', 'Meeting segment player')}
            onPlay={() => setPlaying(true)}
            onError={() => {
              if (currentUrl.current === url) setError('回听无法播放，请重新加载片段后再试。')
            }}
          />
          {!playing && !error && (
            <p role="status" className="text-xs text-muted-foreground">
              {t(
                '片段已加载；若未自动播放，请点击播放器的播放按钮。',
                'The segment is loaded. If playback does not start automatically, click Play in the player.',
              )}
            </p>
          )}
        </>
      )}
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {meetingMessage(error, t)}
        </p>
      )}
      <Button variant="outline" disabled={pending} onClick={() => void load()}>
        <PiPlay />
        {pending
          ? t('正在加载片段…', 'Loading segment…')
          : url || error
            ? t('重新加载片段', 'Reload segment')
            : t('回听本段（最多 30 秒）', 'Play this segment (up to 30 seconds)')}
      </Button>
    </div>
  )
}
