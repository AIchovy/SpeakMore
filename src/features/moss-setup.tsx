import { meetingMessage } from './meeting-messages'
import { useText } from '@/lib/app-state'
import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { Button } from '@/components/ui/button'
import { meetingCall, meetingDownloadLabel, meetingError, type MeetingModels } from './meetings-api'

// Shown only after choosing MOSS, never while using another speech model.
export function MossSetup() {
  const t = useText()

  const [data, setData] = useState<MeetingModels | null>(null)
  const [error, setError] = useState('')
  // Polling failures clear themselves on the next successful read; install errors stay.
  const [pollError, setPollError] = useState('')
  const [pending, setPending] = useState(false)
  useEffect(() => {
    let live = true
    const read = () =>
      void meetingCall<MeetingModels>('meeting_models')
        .then((value) => {
          if (!live) return
          setData(value)
          setPollError('')
        })
        .catch((e) => {
          if (live) setPollError(meetingError(e))
        })
    read()
    const timer = setInterval(read, 1500)
    return () => {
      live = false
      clearInterval(timer)
    }
  }, [])
  const modelReady = data?.models.some((m) => m.id === 'moss' && m.ready)
  const speakerReady = data?.models.some((m) => m.id === 'speaker' && m.ready)
  const ready = modelReady && speakerReady && data?.mossRuntime.ready
  const installing =
    data?.downloadModel === 'moss' ||
    data?.downloadModel === 'moss-runtime' ||
    data?.downloadModel === 'speaker'
  const install = async () => {
    setPending(true)
    setError('')
    try {
      if (installing) await meetingCall('meeting_text_cancel')
      else if (modelReady && data?.mossRuntime.ready && !speakerReady)
        await meetingCall('meeting_model_download', { id: 'speaker' })
      else
        await meetingCall(modelReady ? 'meeting_moss_runtime_download' : 'meeting_model_download', {
          id: 'moss',
        })
      setData(await meetingCall('meeting_models'))
    } catch (e) {
      setError(meetingError(e))
    } finally {
      setPending(false)
    }
  }
  // Once everything is installed, earlier install or polling errors no longer apply.
  if (ready) return null
  return (
    <div className="space-y-3 rounded-lg bg-muted/50 p-4">
      <p className="text-xs leading-6 text-muted-foreground">
        {installing && data
          ? meetingDownloadLabel(data, t)
          : modelReady
            ? t(
                '模型已下载，补齐运行环境与说话人识别模型后即可使用。',
                'Model downloaded. Install the runtime to use it.',
              )
            : t(
                '首次使用需下载 MOSS 模型（约 1.83 GB）及运行环境（约 279 MB），之后可离线使用。',
                'First use requires the MOSS model (about 1.83 GB) and runtime (about 279 MB). You can then use it offline.',
              )}
      </p>
      <div className="flex items-center gap-3">
        <Button
          size="sm"
          variant="outline"
          disabled={!data || pending || (!!data.active && !installing)}
          onClick={() => void install()}
        >
          {installing
            ? t('取消安装', 'Cancel installation')
            : modelReady && data?.mossRuntime.ready && !speakerReady
              ? t('安装说话人连续识别', 'Install continuous speaker identification')
              : modelReady
                ? t('安装运行环境', 'Install runtime')
                : t('下载并安装 MOSS', 'Download and install MOSS')}
        </Button>
        <Link to="/settings/models" className="text-xs underline underline-offset-4">
          {t('管理下载', 'Manage downloads')}
        </Link>
      </div>
      {(error || pollError || data?.error) && (
        <p role="alert" className="text-xs text-destructive">
          {meetingMessage(error || pollError || data?.error, t)}
        </p>
      )}
    </div>
  )
}
