import { useRef, useState } from 'react'
import { Link, useSearchParams } from 'react-router-dom'
import { toast } from 'sonner'
import { useApp, useLiveSession, useText } from '@/lib/app-state'
import { audioLevelFraction } from '@/lib/audio-level'
import { errorMessage, type Phase, type Settings, type ResultRecord } from '@/lib/types'
import { Button } from '@/components/ui/button'
import { Choice, Confirm, InlineError } from '@/components/common'
import {
  ArrowRightIcon,
  MicrophoneIcon,
  ModelIcon,
  CheckIcon,
  XIcon,
  TrashIcon,
  SpinnerIcon,
} from '@/components/icons'

import { RecentResults, ResultDetail } from './results'
import { readinessMessage } from './readiness'
import { OverviewActivity } from './overview-activity'
import { PiCalendarBlank, PiSubtitles } from 'react-icons/pi'

const clock = (ms: number) => {
  const seconds = Math.floor(Math.min(300_000, Math.max(0, ms)) / 1000)
  return `${Math.floor(seconds / 60)
    .toString()
    .padStart(2, '0')}:${(seconds % 60).toString().padStart(2, '0')}`
}

// These read the live timer and input level so the page itself does not
// re-render every time the session republishes them.
function SessionClock() {
  const { elapsedMs } = useLiveSession()
  return <span className="text-foreground">{clock(elapsedMs)}</span>
}
function LimitNotice({ recording }: { recording: boolean }) {
  const { elapsedMs } = useLiveSession(),
    t = useText()
  return recording && elapsedMs >= 270_000
    ? t('即将到达 5 分钟，届时自动完成', 'Reaching 5 minutes; dictation will finish automatically')
    : t(
        '单次最长 5 分钟，自然停顿不会结束录音。音频在本机处理。',
        'Up to 5 minutes per recording. Pauses won’t stop it. Audio is processed on this device.',
      )
}
const meterWeights = [0.16, 0.3, 0.5, 0.72, 0.9, 0.65, 0.43, 0.79, 1, 0.73, 0.45, 0.26, 0.13]
const meterHeight = 52
// Fixed-height bars scaled on the compositor; animating height forced layout
// on every frame. The level uses the dBFS scale so normal speech is visible.
function LevelMeter({ recording }: { recording: boolean }) {
  const { audioLevel } = useLiveSession()
  const level = recording ? audioLevelFraction(audioLevel) : 0
  return meterWeights.map((weight, i) => (
    <span
      key={i}
      className="h-[52px] w-[3px] shrink-0 rounded-full bg-current transition-transform duration-150 ease-out"
      style={{
        opacity: i % 3 === 0 ? 0.5 : 0.85,
        transform: `scaleY(${(recording ? 4 + level * 48 * weight : 36 * weight) / meterHeight})`,
      }}
    />
  ))
}

export function Overview() {
  const { data, save, pending, mutate } = useApp(),
    t = useText()
  const [acting, setActing] = useState(false)
  const [clear, setClear] = useState(false)
  const [savedResult, setSavedResult] = useState<ResultRecord | null>(null)
  const [search, setSearch] = useSearchParams()
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const recentTrigger = useRef<HTMLElement | null>(null),
    allResultsButton = useRef<HTMLButtonElement | null>(null)
  const showResults = (id: string | null = null, trigger?: HTMLElement) => {
    recentTrigger.current = trigger ?? allResultsButton.current
    setSelectedId(id)
    setSearch({ results: 'all' })
  }
  if (!data) return null
  const session = data.session
  const active = session.sessionId !== null
  const recording = session.phase === 'recording'
  const refinementLabel = data.settings.polishEnabled
    ? t('按应用与网站整理', 'Adapt to app and website')
    : t('保持原文', 'Original wording')
  const ending = ['cancelling', 'finishing', 'delivery_check', 'submitting'].includes(session.phase)
  const readinessLink = {
    models: ['/settings/models', t('管理模型', 'Manage models')],
    microphone: ['/settings/audio', t('检查麦克风权限', 'Check microphone access')],
    device: ['/settings/audio', t('选择麦克风', 'Choose microphone')],
  }[data.readinessAction as 'models' | 'microphone' | 'device']
  const labels: Record<Phase, string> = {
    idle: data.canStart
      ? t('说出你的想法', 'Speak your mind')
      : t('准备开始听写', 'Get ready to dictate'),
    preparing: t('正在连接麦克风', 'Connecting your microphone'),
    recording: t('正在聆听', 'Listening'),
    finalizing_audio: t('正在整理音频', 'Preparing your audio'),
    transcribing: t('正在转写', 'Transcribing'),
    polishing: t('正在整理文字', 'Refining your words'),
    delivery_check: t('文字已准备好', 'Your words are ready'),
    submitting: t('正在输入', 'Inserting your words'),
    cancelling: t('正在取消', 'Cancelling'),
    finishing: t('正在结束听写', 'Finishing'),
  }
  async function perform(command: string, args: Record<string, unknown>) {
    setActing(true)
    try {
      await mutate(command, args)
    } catch (error) {
      toast.error(errorMessage(error, t))
    } finally {
      setActing(false)
    }
  }
  return (
    <>
      <header className="mb-6">
        <h1 className="text-[28px] font-medium tracking-tight">
          {t('想说的，现在就说。', 'Say what’s on your mind.')}
        </h1>
        <p className="mt-2 text-sm text-muted-foreground">
          {t(
            '听写想法，记录会议，跟上每一句。',
            'Dictate thoughts, record meetings, and follow every word.',
          )}
        </p>
      </header>
      <section
        className="overflow-hidden rounded-lg bg-muted/80"
        aria-label={t('听写', 'Dictation')}
      >
        <div className="flex flex-wrap items-center justify-between gap-4 px-6 py-5">
          <div className="min-w-0 flex-1 basis-60">
            <div className="min-w-0">
              <div className="mb-2 flex flex-wrap items-center gap-2.5" role="status">
                <h2 className="text-xl font-medium tracking-tight">{labels[session.phase]}</h2>
              </div>
              {active ? (
                <p className="text-sm tabular-nums text-muted-foreground">
                  <SessionClock /> / 05:00
                </p>
              ) : (
                <p className="max-w-[380px] text-sm leading-6 text-muted-foreground">
                  {data.canStart
                    ? data.shortcutNeedsRelease
                      ? t(
                          '请先松开快捷键，再按一下开始听写。',
                          'Release the shortcut, then press once to start.',
                        )
                      : t(
                          '点击开始，说完后完成。文字会出现在下方。',
                          'Start speaking, then finish. Your words will appear below.',
                        )
                    : !data.desktop
                      ? t(
                          '在 Mac 应用中选择本地模型，即可开始。',
                          'Choose a local model in the Mac app to get started.',
                        )
                      : readinessMessage(data, t)}
                </p>
              )}
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-5">
            <div
              className="flex h-10 w-24 items-center justify-center gap-1 text-foreground"
              aria-hidden
            >
              {active && !recording ? (
                <SpinnerIcon className="size-7 animate-spin motion-reduce:animate-none" />
              ) : (
                <LevelMeter recording={recording} />
              )}
            </div>
            <div className="flex flex-wrap justify-center gap-2">
              {active ? (
                <>
                  <Button
                    variant="outline"
                    disabled={acting || ending}
                    onClick={() => void perform('cancel_session', { sessionId: session.sessionId })}
                  >
                    <XIcon aria-hidden />
                    {t('取消', 'Cancel')}
                  </Button>
                  {recording && (
                    <Button
                      disabled={acting}
                      onClick={() => void perform('stop_session', { sessionId: session.sessionId })}
                    >
                      <CheckIcon aria-hidden />
                      {t('完成听写', 'Finish')}
                    </Button>
                  )}
                </>
              ) : (
                <>
                  {data.canStart ? (
                    <Button
                      disabled={acting || pending}
                      onClick={() =>
                        void perform('start_session', {
                          requestId: crypto.randomUUID(),
                          mode: 'toggle',
                        })
                      }
                    >
                      <MicrophoneIcon aria-hidden />
                      {t('开始听写', 'Start dictation')}
                    </Button>
                  ) : readinessLink ? (
                    <Button asChild variant="outline">
                      <Link to={readinessLink[0]}>
                        {readinessLink[1]}
                        <ArrowRightIcon aria-hidden />
                      </Link>
                    </Button>
                  ) : null}
                </>
              )}
            </div>
          </div>
        </div>
        <div className="mx-6 space-y-1 border-t py-3 text-xs leading-5 text-muted-foreground sm:mx-8">
          <p>
            {t(
              '在此听写后复制文字；在其他应用中使用快捷键，可直接输入。',
              'Copy text after dictating here. Use the shortcut in another app to insert it directly.',
            )}
          </p>
          <p>
            <LimitNotice recording={recording} />
          </p>
        </div>
      </section>
      {data.lastError && (
        <div className="mt-4">
          <InlineError message={data.lastError} />
        </div>
      )}
      {data.storageWarning && (
        <p role="alert" className="mt-4 text-sm text-destructive">
          {errorMessage(data.storageWarning, t)}
        </p>
      )}
      <div className="mt-3 grid gap-3 sm:grid-cols-2">
        <Link
          to="/meetings?start=1"
          className="flex items-center gap-3 rounded-lg border px-4 py-3 hover:bg-muted/50 focus-visible:outline-2 focus-visible:outline-offset-2"
        >
          <PiCalendarBlank className="size-5 text-muted-foreground" aria-hidden />
          <span className="flex-1 text-sm">{t('开始会议', 'Start a meeting')}</span>
          <ArrowRightIcon className="size-4 text-muted-foreground" aria-hidden />
        </Link>
        <Link
          to="/captions"
          className="flex items-center gap-3 rounded-lg border px-4 py-3 hover:bg-muted/50 focus-visible:outline-2 focus-visible:outline-offset-2"
        >
          <PiSubtitles className="size-5 text-muted-foreground" aria-hidden />
          <span className="flex-1 text-sm">{t('实时字幕', 'Live captions')}</span>
          <ArrowRightIcon className="size-4 text-muted-foreground" aria-hidden />
        </Link>
      </div>
      <OverviewActivity
        onResult={(record, trigger) => {
          recentTrigger.current = trigger
          if (data.recentResults.some((r) => r.sessionId === record.sessionId)) {
            setSavedResult(null)
            showResults(record.sessionId, trigger)
          } else {
            setSavedResult(record)
          }
        }}
      />
      {data.recentResults.length > 0 && (
        <div className="mt-4 flex flex-wrap items-center justify-between gap-3">
          <p className="text-xs text-muted-foreground">
            {t(
              '临时结果保留 15 分钟；开启历史记录后可长期回看。',
              'Temporary results last 15 minutes. Enable history to read them later.',
            )}
          </p>
          <div className="flex flex-wrap gap-1">
            <Button
              ref={allResultsButton}
              size="sm"
              variant="ghost"
              onClick={() => {
                setSavedResult(null)
                showResults()
              }}
            >
              {t('查看全部最近结果', 'All recent results')}
            </Button>
            <Button
              size="sm"
              variant="ghost"
              className="text-muted-foreground"
              onClick={() => setClear(true)}
            >
              <TrashIcon aria-hidden />
              {t('清空临时结果', 'Clear temporary results')}
            </Button>
          </div>
        </div>
      )}
      <details className="my-6 rounded-lg border px-4 py-3">
        <summary className="cursor-pointer text-xs text-muted-foreground">
          {t('听写设置', 'Dictation settings')}
        </summary>
        <div className="mt-5 grid gap-6 sm:grid-cols-3 sm:gap-7">
          <div className="min-w-0 space-y-2">
            <span className="text-xs text-muted-foreground">
              {t('识别语言', 'Speech language')}
            </span>
            <Choice
              label={t('识别语言', 'Speech language')}
              value={data.settings.language}
              onChange={(v) =>
                void save({ language: v as Settings['language'] }).catch((e) =>
                  toast.error(errorMessage(e, t)),
                )
              }
              disabled={pending || active}
              options={[
                ['auto', t('自动检测', 'Auto-detect')],
                ['zh', t('中文', 'Chinese')],
                ['en', t('英文', 'English')],
              ]}
              className="w-full border-transparent bg-transparent px-0 shadow-none hover:bg-muted dark:bg-transparent dark:hover:bg-muted"
            />
          </div>
          <div className="min-w-0 space-y-2 sm:border-l sm:pl-7">
            <span className="text-xs text-muted-foreground">
              {active && data.settings.polishEnabled
                ? t('下次听写的文字整理', 'Refinement for next dictation')
                : t('文字整理', 'Text refinement')}
            </span>
            <Button
              asChild
              variant="outline"
              className="w-full justify-between border-transparent bg-transparent px-0 font-normal shadow-none hover:bg-muted dark:bg-transparent dark:hover:bg-muted"
            >
              <Link to="/modes">
                {refinementLabel}
                <ArrowRightIcon className="text-muted-foreground" aria-hidden />
              </Link>
            </Button>
          </div>
          <div className="min-w-0 space-y-2 sm:border-l sm:pl-7">
            <span className="text-xs text-muted-foreground">
              {t('当前语音模型', 'Voice model')}
            </span>
            <Button
              asChild
              variant="outline"
              className="w-full justify-between border-transparent bg-transparent px-0 font-normal shadow-none hover:bg-muted dark:bg-transparent dark:hover:bg-muted"
            >
              <Link to="/settings/models">
                <span className="truncate">{data.modelName ?? t('尚未选择', 'Not selected')}</span>
                <ModelIcon className="shrink-0 text-muted-foreground" aria-hidden />
              </Link>
            </Button>
          </div>
        </div>
      </details>
      <RecentResults
        records={data.recentResults}
        restoreFocus={() =>
          (recentTrigger.current?.isConnected
            ? recentTrigger.current
            : allResultsButton.current
          )?.focus()
        }
        open={search.get('results') === 'all'}
        selectedId={selectedId}
        onSelect={setSelectedId}
        onClose={() => {
          setSelectedId(null)
          setSearch({}, { replace: true })
        }}
      />
      <ResultDetail
        record={savedResult}
        onClose={() => setSavedResult(null)}
        restoreFocus={() =>
          (recentTrigger.current?.isConnected
            ? recentTrigger.current
            : allResultsButton.current
          )?.focus()
        }
      />
      <Confirm
        open={clear}
        onOpenChange={setClear}
        title={t('清空最近结果？', 'Clear recent results?')}
        description={t(
          '将清除内存中的临时文字，已保存的历史记录和使用次数保持不变。',
          'Removes temporary results from memory. Saved history and usage counts stay as they are.',
        )}
        onConfirm={() => mutate('clear_recent_results', {})}
      />
    </>
  )
}
