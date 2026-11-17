import { meetingMessage } from './meeting-messages'
import { useText } from '@/lib/app-state'
import { TextServiceChoice } from './text-service-choice'
import { ExtendedModels } from './meeting-extras'
import { useEffect, useState } from 'react'
import { isTauri } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { Button } from '@/components/ui/button'
import { meetingCall, meetingError, type MeetingList } from './meetings-api'
import { PiDownloadSimple, PiTrash } from 'react-icons/pi'

export function MeetingModel() {
  const t = useText()

  const [data, setData] = useState<MeetingList | null>(null)
  const [error, setError] = useState<string | null>(null)
  // Polling failures clear themselves on the next successful read; action errors stay.
  const [pollError, setPollError] = useState<string | null>(null)
  const [pending, setPending] = useState(false)
  // Only this card's model reports progress through the meeting list.
  const downloading = data?.downloadModel === 'qwen3'
  useEffect(() => {
    // Downloads, jobs and recordings announce their start and end with meetings_changed, so the
    // full meeting list is only polled while a download reports progress.
    let live = true,
      reading = false,
      again = false
    let off: (() => void) | undefined
    const read = () => {
      if (!live) return
      if (reading) {
        again = true
        return
      }
      reading = true
      void meetingCall<MeetingList>('meeting_list')
        .then((d) => {
          if (!live) return
          setData(d)
          setPollError(null)
        })
        .catch((e) => {
          if (live) setPollError(meetingError(e))
        })
        .finally(() => {
          reading = false
          if (live && again) {
            again = false
            read()
          }
        })
    }
    read()
    const timer = downloading ? setInterval(read, 2000) : undefined
    if (isTauri())
      void listen('meetings_changed', read).then((fn) => {
        if (live) off = fn
        else fn()
      })
    return () => {
      live = false
      clearInterval(timer)
      off?.()
    }
  }, [downloading])
  const act = async (command: string) => {
    setPending(true)
    setError(null)
    try {
      await meetingCall(command)
      setData(await meetingCall('meeting_list'))
      setPollError(null)
    } catch (e) {
      setError(meetingError(e))
    } finally {
      setPending(false)
    }
  }
  return (
    <>
      <TextServiceChoice />
      <section className="mt-10 border-t pt-7">
        <h2 className="text-base font-medium">{t('本地文字模型', 'Local text model')}</h2>
        <div className="mt-4 flex flex-wrap items-center justify-between gap-4 rounded-xl bg-muted/40 p-5">
          <div>
            <p className="text-sm font-medium">
              Qwen3 4B{' '}
              <span className="ml-2 text-xs font-normal text-muted-foreground">
                Q4_K_M · 2.5 GB · Apache 2.0
              </span>
            </p>
            <p className="mt-2 text-xs leading-6 text-muted-foreground">
              {t(
                '在本机整理会议纪要、翻译实时字幕。与语音转写模型配合使用。',
                'Summarize meetings and translate live captions on your device. Works alongside speech transcription models.',
              )}
            </p>
            {data?.textModelReady && (
              <p className="mt-2 text-xs">
                {t('已安装 · 会议整理时自动使用', 'Installed · Available for local summaries')}
              </p>
            )}
          </div>
          {data?.downloadModel === 'qwen3' ? (
            <div className="flex items-center gap-3">
              <span className="text-xs tabular-nums">
                {Math.floor((data.downloadedBytes / data.textModelBytes) * 100)}%
              </span>
              <Button
                size="sm"
                variant="outline"
                disabled={pending}
                onClick={() => void act('meeting_text_cancel')}
              >
                {t('取消下载', 'Cancel download')}
              </Button>
            </div>
          ) : (
            <Button
              size="sm"
              variant="outline"
              disabled={
                !data || pending || !!data.downloadModel || (data.textModelReady && !!data.active)
              }
              onClick={() =>
                void act(data?.textModelReady ? 'meeting_text_remove' : 'meeting_text_download')
              }
            >
              {data?.textModelReady ? <PiTrash /> : <PiDownloadSimple />}
              {data?.textModelReady
                ? t('移除模型', 'Remove model')
                : t('下载模型', 'Download model')}
            </Button>
          )}
        </div>
        {(error || pollError || data?.downloadError) && (
          <p role="alert" className="mt-3 text-sm text-destructive">
            {meetingMessage(error || pollError || data?.downloadError, t)}
          </p>
        )}
      </section>
      <ExtendedModels />
    </>
  )
}
