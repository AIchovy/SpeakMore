import { meetingMessage } from './meeting-messages'
import { useText } from '@/lib/app-state'
import { useCallback, useEffect, useRef, useState } from 'react'
import { isTauri } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { PiArrowUp, PiCopy, PiStop, PiTrash, PiSparkle } from 'react-icons/pi'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from '@/components/ui/dialog'
import { meetingCall, meetingError, timecode } from './meetings-api'

interface Evidence {
  id: string
  startMs: number
  text: string
  source: string
}
interface Message {
  role: 'user' | 'assistant'
  text: string
  at: number
  evidence: Evidence[]
  stale?: boolean
}
interface Conversation {
  sessionId: string
  messages: Message[]
  busy: boolean
  phase: string | null
  hasMemory: boolean
  persistent: boolean
  sourceCount: number
}

export function ConversationAssistant({
  meetingId,
  contextKey,
  surface,
}: {
  meetingId?: string
  contextKey: string
  surface?: 'meeting' | 'captions'
}) {
  // A new session gets its own component so a late response cannot replace another conversation.
  return <AssistantSession key={contextKey} meetingId={meetingId} surface={surface} />
}
function AssistantSession({
  meetingId,
  surface,
}: {
  meetingId?: string
  surface?: 'meeting' | 'captions'
}) {
  const t = useText()
  const captions = surface === 'captions' || !meetingId

  const suggestions = captions
    ? [
        t('概括刚才的内容', 'Summarize the recent content'),
        t('解释其中的术语', 'Explain the terms used'),
        t('整理学习要点', 'Organize the learning points'),
      ]
    : [
        t('回顾刚才讨论的内容', 'Recap what we just discussed'),
        t('有哪些待办需要确认？', 'Which action items need confirmation?'),
        t('我可以怎么回复？', 'How could I respond?'),
      ]
  const [conversation, setConversation] = useState<Conversation | null>(null)
  const [draft, setDraft] = useState('')
  const [sending, setSending] = useState(false)
  const [submitted, setSubmitted] = useState('')
  const [error, setError] = useState('')
  const [confirmClear, setConfirmClear] = useState(false)
  const [copied, setCopied] = useState<number | null>(null)
  const [clearing, setClearing] = useState(false)
  const live = useRef(false)
  const request = useRef(0)
  const scroll = useRef<HTMLDivElement>(null)
  const follow = useRef(true)
  const input = useRef<HTMLTextAreaElement>(null)
  const args = { meetingId: meetingId ?? null }
  const refresh = useCallback(async () => {
    const version = ++request.current
    try {
      const view = await meetingCall<Conversation>('assistant_view', {
        meetingId: meetingId ?? null,
      })
      if (live.current && version === request.current) setConversation(view)
    } catch (e) {
      if (live.current && version === request.current) setError(meetingError(e))
    }
  }, [meetingId])
  useEffect(() => {
    live.current = true
    // Per-effect flag: under StrictMode a late listen() from the first run must not survive.
    let disposed = false
    const subscriptions: (() => void)[] = []
    void refresh()
    if (isTauri())
      for (const event of ['assistant_changed', 'meetings_changed'])
        void listen(event, () => void refresh()).then((unlisten) => {
          if (disposed) unlisten()
          else subscriptions.push(unlisten)
        })
    return () => {
      disposed = true
      live.current = false
      request.current++
      subscriptions.forEach((off) => off())
    }
  }, [refresh])
  useEffect(() => {
    if (follow.current && scroll.current) scroll.current.scrollTop = scroll.current.scrollHeight
  }, [conversation?.messages.length, sending, conversation?.phase])
  const busy = sending || !!conversation?.busy || clearing
  const send = async () => {
    const question = draft.trim()
    if (!question || busy) return
    if (new TextEncoder().encode(question).length > 1500) {
      setError(
        t(
          '问题有些长，请缩短到约 500 个汉字。',
          'Your question is too long. Keep it under 1,500 UTF-8 bytes (about 1,500 English characters).',
        ),
      )
      return
    }
    setSending(true)
    setSubmitted(question)
    setError('')
    follow.current = true
    try {
      await meetingCall('assistant_ask', { ...args, question, sessionId: conversation?.sessionId })
      if (live.current) setDraft((value) => (value.trim() === question ? '' : value))
    } catch (e) {
      if (live.current) setError(meetingError(e))
    } finally {
      if (live.current) {
        setSending(false)
        setSubmitted('')
        await refresh()
        input.current?.focus()
      }
    }
  }
  const copy = async (message: Message, index: number) => {
    try {
      await meetingCall('captions_copy', { text: message.text })
      setCopied(index)
    } catch (e) {
      setError(meetingError(e))
    }
  }
  return (
    <section className="mx-auto w-full max-w-[880px]" aria-label={t('AI 助手', 'AI assistant')}>
      <div className="mb-5 flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="flex items-center gap-2 text-base font-medium">
            <PiSparkle aria-hidden />
            {captions ? t('字幕助手', 'Caption assistant') : t('AI 助手', 'AI assistant')}
          </h2>
          <p className="mt-1.5 text-xs leading-6 text-muted-foreground">
            {meetingId
              ? t(
                  '使用本次会议的 AI 服务，结合逐字稿与实时字幕回答。',
                  'Uses this meeting’s AI service to answer from the transcript and live captions.',
                )
              : t(
                  '根据本次已识别的字幕回答，适用于视频、课程、播客和实时对话。',
                  'Answers from recognized captions in this session, for videos, lessons, podcasts and live conversations.',
                )}
          </p>
        </div>
        {!!conversation?.messages.length && (
          <Button variant="ghost" size="sm" disabled={busy} onClick={() => setConfirmClear(true)}>
            <PiTrash />
            {t('清空对话', 'Clear conversation')}
          </Button>
        )}
      </div>
      <div
        ref={scroll}
        onScroll={() => {
          const el = scroll.current
          if (el) follow.current = el.scrollHeight - el.scrollTop - el.clientHeight < 60
        }}
        className="max-h-[520px] min-h-[200px] overflow-y-auto overscroll-contain pr-2"
        aria-label={t('对话记录', 'Conversation history')}
        role="log"
        aria-live="polite"
        aria-relevant="additions"
      >
        {!conversation?.messages.length && !sending && (
          <div className="py-8">
            <p className="text-xl font-medium tracking-tight">
              {captions
                ? t('看看这段内容。', 'Explore this content.')
                : t('接着会议，继续聊。', 'Keep the conversation going.')}
            </p>
            <p className="mt-3 max-w-lg text-sm leading-7 text-muted-foreground">
              {captions
                ? t(
                    '概括刚才听到的内容、解释专业词，或梳理学习要点。可以继续追问，回答下方附有参考的字幕原文。',
                    'Summarize what you just heard, explain a term or organize learning points. Ask follow-up questions and review the caption excerpts below each answer.',
                  )
                : t(
                    '回顾讨论、解释术语，或者准备下一句回复。你可以继续追问，回答下方可查看本次参考的原文。',
                    'Recap the discussion, explain a term or prepare your next reply. Ask follow-up questions and review the source excerpts below each answer.',
                  )}
            </p>
            <div className="mt-6 flex flex-wrap gap-2">
              {suggestions.map((text) => (
                <Button
                  key={text}
                  variant="outline"
                  size="sm"
                  disabled={busy}
                  onClick={() => {
                    setDraft(text)
                    input.current?.focus()
                  }}
                >
                  {text}
                </Button>
              ))}
            </div>
          </div>
        )}
        {conversation?.messages.map((message, index) => (
          <article
            key={`${index}-${message.at}`}
            className={
              message.role === 'user'
                ? 'mb-6 ml-auto w-fit max-w-[85%] rounded-2xl bg-muted px-4 py-3'
                : 'mb-8 max-w-[760px]'
            }
          >
            <p className="mb-1 text-xs text-muted-foreground">
              {message.role === 'user' ? t('你', 'You') : 'SpeakMore'}
            </p>
            {message.stale && (
              <p role="status" className="mb-2 text-xs text-muted-foreground">
                {t(
                  '这条回答参考的原文已有修改，请重新提问核对。',
                  'The transcript used for this answer has changed. Ask again to check it.',
                )}
              </p>
            )}
            <p className="whitespace-pre-wrap break-words text-sm leading-7">{message.text}</p>
            {message.role === 'assistant' && (
              <div className="mt-2">
                <Button
                  variant="ghost"
                  size="sm"
                  className="-ml-2 text-muted-foreground"
                  onClick={() => void copy(message, index)}
                >
                  <PiCopy />
                  {copied === index ? t('已复制', 'Copied') : t('复制回答', 'Copy answer')}
                </Button>
                {!!message.evidence.length && (
                  <details className="mt-2 rounded-lg border px-3 py-2 text-xs">
                    <summary className="cursor-pointer py-1 text-muted-foreground">
                      {t('本次参考原文 ·', 'Source excerpts ·')}
                      {message.evidence.length} {t('个片段', 'excerpts')}
                    </summary>
                    <div className="mt-2 divide-y">
                      {message.evidence.map((e, i) => (
                        <div key={`${e.id}-${i}`} className="py-3">
                          <span className="text-muted-foreground">
                            {meetingMessage(e.source, t)} {timecode(e.startMs)}
                          </span>
                          <p className="mt-1 whitespace-pre-wrap break-words leading-6">{e.text}</p>
                        </div>
                      ))}
                    </div>
                  </details>
                )}
              </div>
            )}
          </article>
        ))}
        {sending && submitted && (
          <p className="mb-4 ml-auto w-fit max-w-[85%] whitespace-pre-wrap rounded-2xl bg-muted px-4 py-3 text-sm leading-7">
            {submitted}
          </p>
        )}
        {(sending || conversation?.busy) && (
          <p role="status" className="pb-6 text-sm text-muted-foreground">
            {conversation?.phase === 'compacting'
              ? t('正在整理较早的对话…', 'Summarizing earlier conversation…')
              : t('正在结合原文思考…', 'Thinking with your source material…')}
          </p>
        )}
      </div>
      {error && (
        <p role="alert" className="mb-3 text-sm text-destructive">
          {meetingMessage(error, t)}
        </p>
      )}
      <form
        className="rounded-xl border bg-background p-2 focus-within:ring-1 focus-within:ring-foreground/20"
        onSubmit={(event) => {
          event.preventDefault()
          void send()
        }}
      >
        <Textarea
          ref={input}
          aria-label={t('向 AI 助手提问', 'Ask the AI assistant')}
          placeholder={
            captions
              ? t('关于这段内容，问点什么…', 'Ask about this content…')
              : t('关于刚才的内容，问点什么…', 'Ask about what was just discussed…')
          }
          className="min-h-20 max-h-44 resize-none border-0 shadow-none focus-visible:ring-0"
          value={draft}
          maxLength={1500}
          disabled={sending || clearing}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (
              event.key === 'Enter' &&
              !event.shiftKey &&
              !event.nativeEvent.isComposing &&
              event.keyCode !== 229
            ) {
              event.preventDefault()
              void send()
            }
          }}
        />
        <div className="flex items-center justify-between gap-3 px-1 pt-1">
          <span className="text-[11px] text-muted-foreground">
            {conversation?.hasMemory
              ? t(
                  '早期对话已整理，原始记录保留',
                  'Earlier conversation summarized; original history kept',
                )
              : t(
                  'Enter 发送 · Shift + Enter 换行',
                  'Enter to send · Shift + Enter for a new line',
                )}
          </span>
          {conversation?.busy ? (
            <Button
              type="button"
              size="sm"
              variant="outline"
              onClick={() => {
                void meetingCall('assistant_cancel', { sessionId: conversation.sessionId }).catch(
                  (e) => setError(meetingError(e)),
                )
              }}
            >
              <PiStop />
              {t('停止回答', 'Stop response')}
            </Button>
          ) : (
            <Button type="submit" size="sm" disabled={!draft.trim() || busy || !conversation}>
              <PiArrowUp />
              {t('发送', 'Send')}
            </Button>
          )}
        </div>
      </form>
      <p className="mt-3 text-xs leading-6 text-muted-foreground">
        {conversation?.persistent
          ? t('对话保存在本机。', 'Conversation is saved on your device. ')
          : t(
              '本次对话仅暂存；下次开始前可开启“保存字幕文字”。',
              'This conversation is temporary. Enable “Save caption text” before your next session to keep it. ',
            )}
        {meetingId
          ? t('与本次会议关联的字幕共用对话。', 'Linked captions share this conversation. ')
          : t(
              '当前对话使用本次字幕内容。',
              'This conversation uses content from this caption session. ',
            )}
        {t(
          'AI 服务收到相关原文与对话上下文。',
          'The AI service receives relevant source text and conversation context.',
        )}
      </p>
      <Dialog open={confirmClear} onOpenChange={setConfirmClear}>
        <DialogContent closeLabel={t('关闭', 'Close')}>
          <DialogHeader>
            <DialogTitle>{t('清空这段对话？', 'Clear this conversation?')}</DialogTitle>
            <DialogDescription>
              {t(
                '删除问答和上下文摘要，会议逐字稿与字幕原文仍然保留。关联的会议和字幕页面会同步清空。',
                'Deletes questions, answers and context summaries. Meeting transcripts and captions are kept. The conversation is also cleared from linked meeting and caption pages.',
              )}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setConfirmClear(false)}>
              {t('取消', 'Cancel')}
            </Button>
            <Button
              variant="destructive"
              disabled={clearing}
              onClick={() => {
                setClearing(true)
                void meetingCall('assistant_clear', args)
                  .then(async () => {
                    setConfirmClear(false)
                    setError('')
                    await refresh()
                  })
                  .catch((e) => setError(meetingError(e)))
                  .finally(() => setClearing(false))
              }}
            >
              {t('清空对话', 'Clear conversation')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  )
}
