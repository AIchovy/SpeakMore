import { useEffect, useState, useSyncExternalStore } from 'react'
import { ArrowRightIcon, CheckIcon, XIcon, WaveformIcon } from '@/components/icons'
import { Button } from '@/components/ui/button'
import { audioLevelFraction } from '@/lib/audio-level'
import { errorMessage, type RecordingStatus } from '@/lib/types'

// The HUD draws five bars with CSS transforms; it does not load the motion library.
const reducedMotionQuery = '(prefers-reduced-motion: reduce)'
const subscribeReducedMotion = (change: () => void) => {
  const media = window.matchMedia(reducedMotionQuery)
  media.addEventListener('change', change)
  return () => media.removeEventListener('change', change)
}
const prefersReducedMotion = () => window.matchMedia(reducedMotionQuery).matches
const levelWeights = [0.5, 0.85, 1, 0.7, 0.45]

export function RecordingBar({
  status,
  act,
}: {
  status: RecordingStatus
  act: (cancel: boolean) => Promise<void>
}) {
  const [pending, setPending] = useState(false)
  const [failed, setFailed] = useState(false)
  const reduced =
    useSyncExternalStore(subscribeReducedMotion, prefersReducedMotion, () => false) ||
    status.motionPolicy === 'reduced'
  const level = audioLevelFraction(status.audioLevel)
  const english = status.uiLocale === 'en'
  const recording = status.session.phase === 'recording'
  const ending = ['cancelling', 'finishing'].includes(status.session.phase)
  const seconds = Math.min(300, Math.floor(status.session.elapsedMs / 1000))
  const time = `${String(Math.floor(seconds / 60)).padStart(2, '0')}:${String(seconds % 60).padStart(2, '0')}`
  const delivery = status.notice?.deliveryStatus
  const fallback: Record<string, [string, string]> = {
    accessibility_required: ['需要辅助功能权限', 'Accessibility access required'],
    unsupported_target: ['此输入框暂不支持', 'Unsupported input field'],
    read_only_target: ['输入框不可编辑', 'Field is not editable'],
    sensitive_target: ['受保护的输入框', 'Protected input field'],
    target_changed: ['输入位置已变化', 'Input target changed'],
    target_unavailable: ['无法读取原输入框', 'Cannot read the original field'],
    probe_timeout: ['输入框检查超时', 'Input field check timed out'],
    needs_review: ['识别结果需要检查', 'Review the transcription'],
    text_too_long: ['文字较长，请手动粘贴', 'Long text: paste manually'],
    user_requested: ['已设置为手动输入', 'Manual delivery selected'],
  }
  const reason = fallback[status.notice?.deliveryReason ?? '']
  const reasonText = reason ? reason[english ? 1 : 0] : null
  const delivered = delivery === 'submitted' || delivery === 'verified'
  const resultHeading = delivered
    ? english
      ? 'Paste submitted'
      : '已提交粘贴'
    : delivery === 'uncertain'
      ? english
        ? 'Check the target app'
        : '请检查原应用'
      : delivery === 'copied_only'
        ? english
          ? 'Copied · Paste manually'
          : '已复制，请手动粘贴'
        : (reasonText ?? (english ? 'Result is ready' : '结果已保留'))
  const resultDetail = delivered
    ? english
      ? 'View text in the original app'
      : '请在原输入框查看文字'
    : delivery === 'uncertain'
      ? english
        ? 'Check before pasting again'
        : '请确认是否已输入，避免重复'
      : delivery === 'copied_only'
        ? english
          ? 'Press ⌘V in your target field'
          : '在目标输入框按 ⌘V'
        : english
          ? 'Open to copy'
          : '打开应用后复制'
  const text = status.notice
    ? status.notice.hasResult
      ? resultHeading
      : errorMessage(status.notice.error, (zh, en) => (english ? en : zh))
    : status.preview
      ? english
        ? 'Recording bar preview'
        : '悬浮条预览'
      : recording
        ? seconds >= 270
          ? english
            ? `${300 - seconds}s remaining`
            : `还可录音 ${300 - seconds} 秒`
          : english
            ? 'Listening'
            : '正在听写'
        : status.session.phase === 'preparing'
          ? english
            ? 'Preparing microphone'
            : '正在准备麦克风'
          : ending
            ? english
              ? 'Finishing'
              : '正在结束'
            : seconds >= 300
              ? english
                ? '5-minute limit · Transcribing'
                : '已达 5 分钟，正在转写'
              : english
                ? 'Transcribing on device'
                : '正在本地转写'
  useEffect(() => {
    setFailed(false)
    setPending(false)
  }, [status.session.sessionId, status.preview, status.notice?.sessionId])
  const perform = async (cancel: boolean) => {
    setPending(true)
    setFailed(false)
    try {
      await act(cancel)
    } catch {
      setFailed(true)
    } finally {
      setPending(false)
    }
  }
  return (
    <div
      className="recording-bar"
      role="group"
      aria-label={english ? 'Recording controls' : '录音控制'}
    >
      <Button
        variant="ghost"
        size="icon"
        className="recording-cancel"
        disabled={pending || (!status.preview && ending)}
        aria-label={
          status.notice
            ? english
              ? 'Dismiss message'
              : '关闭提示'
            : status.preview
              ? english
                ? 'Close preview'
                : '关闭预览'
              : english
                ? 'Cancel dictation'
                : '取消听写'
        }
        onClick={() => void perform(true)}
      >
        <XIcon aria-hidden />
      </Button>
      <div className="recording-body">
        <div className="recording-heading" role="status" title={text}>
          {failed ? (english ? 'Use the menu bar to cancel' : '请从菜单栏取消') : text}
        </div>
        <div className="recording-detail">
          {status.notice ? (
            <span className="text-[11px] text-[#a9beb8]">
              {status.notice.hasResult
                ? resultDetail
                : english
                  ? 'Open app for details'
                  : '打开应用查看详情'}
            </span>
          ) : (
            <>
              {recording && !reduced ? (
                <span className="recording-level" aria-hidden>
                  {levelWeights.map((weight, index) => (
                    <i
                      key={index}
                      style={{ transform: `scaleY(${Math.max(0.13, level * weight)})` }}
                    />
                  ))}
                </span>
              ) : (
                <WaveformIcon aria-hidden className="size-4" />
              )}
              <span className={seconds >= 270 ? 'recording-time near-limit' : 'recording-time'}>
                {time}
                <span> / 05:00</span>
              </span>
            </>
          )}
        </div>
      </div>
      <Button
        size="icon"
        className="recording-finish"
        disabled={pending || (!status.notice && (!recording || status.preview))}
        aria-label={
          status.notice
            ? english
              ? 'View result'
              : '查看结果'
            : english
              ? 'Finish recording'
              : '结束录音'
        }
        onClick={() => void perform(false)}
      >
        {status.notice ? <ArrowRightIcon aria-hidden /> : <CheckIcon aria-hidden />}
      </Button>
    </div>
  )
}
