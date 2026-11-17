import { meetingMessage } from './features/meeting-messages'
import React, { useEffect, useRef, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { isTauri } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { PiPause, PiPlay, PiX, PiDotsSix, PiEyeSlash } from 'react-icons/pi'
import { useCaptions, type CaptionLine } from './features/captions'
import { meetingCall, meetingError, timecode } from './features/meetings-api'
import './styles.css'
type Quick = { text: string; lines: CaptionLine[]; generated: boolean }
function Overlay() {
  const { view } = useCaptions()
  const [history, setHistory] = useState(false),
    [historyLines, setHistoryLines] = useState<CaptionLine[]>([]),
    [recap, setRecap] = useState<Quick | null>(null),
    [quick, setQuick] = useState<Quick | null>(null),
    [assistant, setAssistant] = useState(false),
    [pending, setPending] = useState(false),
    [error, setError] = useState(''),
    [term, setTerm] = useState(''),
    [seconds, setSeconds] = useState('60'),
    [focusLine, setFocusLine] = useState(''),
    [copied, setCopied] = useState(false)
  const live = useRef(true),
    request = useRef(0),
    generation = useRef(view?.sessionId),
    assistantRef = useRef(false)
  const historyRows = useRef(new Map<string, HTMLDivElement>())
  const t = (zh: string, en: string) => (view?.uiLocale === 'en' ? en : zh)
  const lines = (view?.displayHeld ? view.heldLines : view?.lines) || []
  const call = (command: string, args: Record<string, unknown>) =>
    void meetingCall(command, args).catch((e) => setError(meetingError(e)))
  useEffect(() => {
    generation.current = view?.sessionId
    request.current++
    setQuick(null)
    setRecap(null)
    setError('')
    setTerm('')
    setHistory(false)
    setPending(false)
  }, [view?.sessionId])
  const runQuick = async (kind: string, range?: number) => {
    if (pending) return
    const token = ++request.current
    setPending(true)
    setError('')
    setRecap(null)
    setHistory(false)
    setCopied(false)
    try {
      const result = await meetingCall<Quick>(
        kind === 'recap' ? 'captions_recap' : 'captions_insight',
        kind === 'recap'
          ? { seconds: range ?? Number(seconds) }
          : { kind, term: kind === 'term' ? term : null },
      )
      if (live.current && token === request.current) {
        if (kind === 'recap') setRecap(result)
        else {
          setQuick(result)
          setAssistant(true)
        }
      }
    } catch (e) {
      if (live.current && token === request.current) setError(meetingError(e))
    } finally {
      if (live.current && token === request.current) setPending(false)
    }
  }
  const runRef = useRef(runQuick)
  runRef.current = runQuick
  assistantRef.current = assistant
  const dismissQuick = () => {
    request.current++
    setPending(false)
    setQuick(null)
    setRecap(null)
    setError('')
  }
  useEffect(() => {
    if (history && focusLine)
      historyRows.current.get(focusLine)?.scrollIntoView({ block: 'center' })
  }, [history, focusLine])
  useEffect(() => {
    live.current = true
    // Per-effect flag: a listener resolving after this effect's cleanup must not survive a remount.
    let disposed = false,
      off: (() => void) | undefined
    if (isTauri())
      void listen<string>('captions_quick_action', (e) => {
        if (e.payload === 'recap') void runRef.current('recap', 60)
        else {
          request.current++
          setPending(false)
          setAssistant(!assistantRef.current)
          setQuick(null)
          setRecap(null)
          setHistory(false)
        }
      }).then((v) => {
        if (disposed) v()
        else off = v
      })
    return () => {
      disposed = true
      live.current = false
      request.current++
      off?.()
    }
  }, [])
  useEffect(() => {
    call('captions_panel_size', { expanded: assistant || !!recap || history })
  }, [assistant, !!recap, history, view?.fontSize, view?.passthrough, view?.opacity])
  const text = quick || recap
  return (
    <main className="h-screen bg-transparent p-2">
      <section
        className="flex h-full flex-col rounded-2xl border border-white/15 px-5 py-3 text-white shadow-xl"
        style={{ backgroundColor: `rgb(12 12 12 / ${view?.opacity ?? 0.92})` }}
      >
        <div className="mb-2 flex shrink-0 flex-wrap items-center justify-between gap-2 text-white/60">
          <button
            aria-label={t('移动字幕', 'Move captions')}
            onMouseDown={(e) => {
              if (e.button === 0) call('captions_window', { action: 'drag' })
            }}
            className="cursor-move p-1"
          >
            <PiDotsSix />
          </button>
          <span className="text-[10px] tracking-wider">
            {view?.displayHeld
              ? t('已暂留 · 继续识别', 'HELD · STILL LISTENING')
              : view?.paused
                ? t('已暂停收音', 'Capture paused')
                : view?.running
                  ? 'SPEAKMORE LIVE'
                  : 'SPEAKMORE'}
          </span>
          <div className="flex items-center gap-3">
            <button
              className="text-xs"
              onClick={() =>
                call('captions_control', { action: view?.displayHeld ? 'live' : 'hold' })
              }
            >
              {view?.displayHeld ? t('回到实时', 'Live') : t('暂留显示', 'Hold display')}
            </button>
            <button
              className="text-xs"
              onClick={() => {
                dismissQuick()
                if (!history) setHistoryLines(structuredClone(view?.lines || []))
                setHistory(!history)
                setAssistant(false)
                setRecap(null)
              }}
            >
              {history ? t('返回', 'Back') : t('回看', 'History')}
            </button>
            <select
              aria-label={t('回顾范围', 'Recap range')}
              className="max-w-20 bg-transparent text-xs"
              value={seconds}
              onChange={(e) => setSeconds(e.target.value)}
            >
              {[30, 60, 120].map((v) => (
                <option className="bg-neutral-900" key={v} value={v}>
                  {v}
                  {t('秒', 's')}
                </option>
              ))}
            </select>
            <button
              className="text-xs"
              disabled={pending || !lines.length}
              onClick={() => void runQuick('recap')}
            >
              {pending ? '…' : t('刚才讲了什么', 'What did I miss?')}
            </button>
            <button
              className="text-xs"
              onClick={() => {
                dismissQuick()
                setAssistant(!assistant)
                setQuick(null)
                setRecap(null)
                setHistory(false)
              }}
            >
              {assistant ? t('隐藏助手', 'Hide assistant') : t('问一下', 'Ask')}
            </button>
            <button
              aria-label={t('隐藏字幕，继续转录', 'Hide captions and keep transcribing')}
              onClick={() => call('captions_window', { action: 'hide' })}
            >
              <PiEyeSlash />
            </button>
            <button
              aria-label={
                view?.paused ? t('继续收音', 'Resume capture') : t('暂停收音', 'Pause capture')
              }
              onClick={() =>
                call('captions_control', { action: view?.paused ? 'resume' : 'pause' })
              }
            >
              {view?.paused ? <PiPlay /> : <PiPause />}
            </button>
            <button
              aria-label={t('结束字幕', 'End captions')}
              onClick={() => {
                call('captions_control', { action: 'stop' })
                call('captions_window', { action: 'hide' })
              }}
            >
              <PiX />
            </button>
          </div>
        </div>
        {error && (
          <p role="status" className="mb-2 shrink-0 text-xs text-amber-200">
            {error}
          </p>
        )}
        {assistant && (
          <div className="mb-3 shrink-0 rounded-lg border border-white/15 p-3">
            <div className="mb-2 flex justify-between gap-3 text-xs">
              <span>
                {t('私人助手 · 回复由你决定是否采用', 'Private assistant · you choose what to use')}
              </span>
              <button
                onClick={() => {
                  dismissQuick()
                  setAssistant(false)
                  setQuick(null)
                  request.current++
                  setPending(false)
                }}
              >
                {t('隐藏私人内容', 'Hide private content')}
              </button>
            </div>
            <div className="flex flex-wrap gap-2 text-xs">
              {[
                ['question', t('刚才问了什么', 'What was asked?')],
                ['conditions', t('有哪些条件', 'What conditions apply?')],
                ['reply', t('怎么简短回复', 'How can I reply?')],
                ['unanswered', t('还有哪些未回答', 'What remains unanswered?')],
              ].map(([kind, label]) => (
                <button
                  key={kind}
                  disabled={pending}
                  className="rounded border border-white/20 px-2 py-1.5 hover:bg-white/10 disabled:opacity-40"
                  onClick={() => void runQuick(kind)}
                >
                  {label}
                </button>
              ))}
            </div>
            <p className="mt-2 text-[10px] leading-5 text-white/50">
              {t(
                '共享整屏前隐藏私人内容；此面板不能保证被所有共享软件排除。',
                'Hide private content before sharing your entire screen; exclusion depends on the sharing software.',
              )}
            </p>
          </div>
        )}
        {text ? (
          <div className="min-h-0 flex-1 overflow-y-auto">
            <div className="mb-3 flex gap-3 text-xs text-white/60">
              <button
                onClick={() => {
                  dismissQuick()
                  setQuick(null)
                  setRecap(null)
                  setAssistant(false)
                }}
              >
                {t('回到实时字幕', 'Back to live captions')}
              </button>
              <button
                onClick={() =>
                  void meetingCall('captions_copy', { text: text.text })
                    .then(() => setCopied(true))
                    .catch((e) => setError(meetingError(e)))
                }
              >
                {copied ? t('已复制', 'Copied') : t('复制', 'Copy')}
              </button>
            </div>
            <p className="whitespace-pre-wrap text-sm leading-7">{text.text}</p>
            <details className="mt-4">
              <summary className="cursor-pointer text-xs text-white/50">
                {t('参考字幕 · 点击定位', 'Source captions · click to locate')}
              </summary>
              {text.lines.map((line) => (
                <button
                  key={line.id}
                  className="mt-3 block w-full text-left text-xs leading-6 text-white/70"
                  onClick={() => {
                    setHistoryLines(text.lines)
                    setFocusLine(line.id)
                    setHistory(true)
                    setRecap(null)
                    setQuick(null)
                    setAssistant(false)
                  }}
                >
                  {timecode(line.startMs)} · {line.original}
                </button>
              ))}
            </details>
          </div>
        ) : history ? (
          <div
            className="min-h-0 flex-1 overflow-y-auto space-y-4"
            onMouseUp={() => {
              const selected = window.getSelection()?.toString().trim() || ''
              if (selected && selected.length <= 120) setTerm(selected)
            }}
          >
            <p className="text-[10px] text-white/50">
              {t('回看已暂留，后续字幕继续记录', 'History held; new captions keep recording')}
            </p>
            {historyLines.map((line) => (
              <div
                key={line.id}
                ref={(node) => {
                  if (node) historyRows.current.set(line.id, node)
                  else historyRows.current.delete(line.id)
                }}
                className={focusLine === line.id ? 'rounded bg-white/10 p-2' : ''}
              >
                <p className="text-[10px] text-white/40">{timecode(line.startMs)}</p>
                <p className="whitespace-pre-wrap text-sm leading-6">{line.original}</p>
                {view?.translationEnabled && line.translated && (
                  <p className="text-sm leading-6 text-white/60">{line.translated}</p>
                )}
              </div>
            ))}
          </div>
        ) : lines.length ? (
          <div
            className="grid min-h-0 flex-1 grid-rows-2 gap-2 text-center"
            style={{ fontSize: view?.fontSize ?? 24 }}
            onMouseUp={() => {
              const selected = window.getSelection()?.toString().trim() || ''
              if (selected && selected.length <= 120) setTerm(selected)
            }}
          >
            {lines.slice(-2).map((line) => (
              <div key={line.id} className="min-h-0 overflow-y-auto">
                <p className="text-[10px] leading-4 text-white/45">
                  {line.source === 'microphone'
                    ? t('麦克风', 'Microphone')
                    : line.source === 'system'
                      ? t('系统声音', 'System audio')
                      : ''}
                  {!line.finalText && ` · ${t('识别中', 'Recognizing')}`}
                </p>
                {(!view?.translationEnabled || view.original || !line.translated) && (
                  <p
                    className={`min-h-[1.4em] whitespace-pre-wrap leading-snug ${line.finalText ? '' : 'text-white/60'}`}
                  >
                    {line.original}
                  </p>
                )}
                {view?.translationEnabled && (
                  <p className="min-h-[1.4em] whitespace-pre-wrap text-[.7em] leading-snug text-white/70">
                    {line.translated ||
                      (line.finalText
                        ? view.translationStatus === 'retrying'
                          ? t(
                              '翻译暂不可用 · 原文继续',
                              'Translation unavailable · original continues',
                            )
                          : t('正在翻译…', 'Translating…')
                        : '\u00a0')}
                  </p>
                )}
                {line.translationWarning && (
                  <p className="text-[10px] text-amber-200">
                    {t('译文数字待核对', 'Check translated numbers')}
                  </p>
                )}
              </div>
            ))}
          </div>
        ) : (
          <div className="flex flex-1 items-center justify-center text-base text-white/60">
            {view?.error
              ? meetingMessage(view.error, t)
              : view?.running
                ? t('正在准备识别…', 'Preparing recognition…')
                : t('字幕已结束', 'Captions ended')}
          </div>
        )}
        {term && !text && (
          <div className="mt-2 flex shrink-0 items-center justify-center gap-3 text-xs">
            <button
              className="rounded border border-white/20 px-3 py-1.5"
              disabled={pending}
              onClick={() => void runQuick('term')}
            >
              {t('解释选中词语', 'Explain selected term')} · {term}
            </button>
            <button aria-label={t('取消词语选择', 'Clear selection')} onClick={() => setTerm('')}>
              <PiX />
            </button>
          </div>
        )}
        {view?.translationEnabled && view.translationStatus === 'retrying' && !text && (
          <p className="mt-1 shrink-0 text-center text-[10px] text-amber-200">
            {t('翻译重试中 · 原文继续显示', 'Retrying translation · original captions continue')}
          </p>
        )}
      </section>
    </main>
  )
}
createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <Overlay />
  </React.StrictMode>,
)
