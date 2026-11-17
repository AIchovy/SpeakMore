// Synthetic UI fixtures, available only through an explicit development URL.
import type { CaptionView } from './captions'
import type { Meeting, MeetingList } from './meetings-api'
import type { MeetingRecordingView } from './meeting-recording-panel'
let recordingPreview: MeetingRecordingView | null = {
  meetingId: '11111111-1111-4111-8111-111111111111',
  title: '产品评审 · 三小时会议',
  source: 'mixed',
  paused: false,
  stopping: false,
  elapsedMs: 10800000,
  levels: [0.035, 0.055],
  uiLocale: 'zh-CN',
  motionPolicy: 'system',
}
const meetings: Meeting[] = [
  {
    id: '11111111-1111-4111-8111-111111111111',
    title: '产品评审 · 三小时会议',
    createdAt: Date.now() - 10800000,
    durationMs: 10800000,
    source: 'mixed',
    state: 'ready',
    error: null,
    revision: 1,
    language: 'zh',
    template: 'review',
    project: '官网发布',
    templatePrompt: '先列决定，再列未决问题和验收条件。',
    dirtySegments: [],
    notes: [
      {
        id: '55555555-5555-4555-8555-555555555555',
        atMs: 7200000,
        text: '这个报价需要确认',
        kind: 'note',
      },
    ],
    artifacts: [],
    gaps: [
      {
        afterMs: 3600000,
        startedAt: Date.now() - 7200000,
        endedAt: Date.now() - 7140000,
        reason: '耳机连接中断',
      },
    ],
    segments: [
      {
        id: 's1',
        startMs: 7200000,
        endMs: 7230000,
        text: '决定下周发布。李明负责更新客户报价，周五前发送。预算五十万，超过需要重新审批。',
        speaker: '李明',
        speakerId: 'p1',
      },
      {
        id: 's2',
        startMs: 10500000,
        endMs: 10530000,
        text: '最后确认：陈雪负责补充验收条件，周三前完成。',
        speaker: '陈雪',
        speakerId: 'p2',
      },
    ],
    items: [
      {
        kind: 'decision',
        text: '下周发布，预算五十万。',
        owner: null,
        due: null,
        evidence: ['s1'],
        done: false,
      },
      {
        kind: 'todo',
        text: '更新客户报价',
        owner: '李明',
        due: '周五前',
        evidence: ['s1'],
        done: false,
      },
      {
        kind: 'todo',
        text: '补充验收条件',
        owner: '陈雪',
        recipient: '李明',
        due: '周三前',
        evidence: ['s2'],
        done: false,
      },
    ],
    audioDeleted: false,
    retentionDays: 7,
    summaryStale: false,
    summaryProviderId: null,
    summaryFollowAi: false,
    processedSamples: 172800000,
    modelId: 'moss',
  },
]
const previous = structuredClone(meetings[0])
previous.id = '44444444-4444-4444-8444-444444444444'
previous.title = '官网发布 · 上次周会'
previous.createdAt = Date.now() - 3 * 86400000
previous.notes = []
previous.segments = [
  {
    id: 'p1',
    startMs: 30000,
    endMs: 60000,
    text: '决定周五上线，预算十五万。',
    speaker: '李明',
    speakerId: 'p1',
  },
]
previous.items = [
  {
    kind: 'decision',
    text: '周五上线，预算十五万',
    owner: null,
    due: null,
    evidence: ['p1'],
    done: false,
    status: 'decision',
    support: 'supported',
  },
]
meetings.push(previous)
const followupPreview =
  typeof window !== 'undefined' &&
  new URLSearchParams(window.location.search).get('meeting-followups') === '1'
if (followupPreview) {
  const day = (offset: number) => {
    const date = new Date()
    date.setDate(date.getDate() + offset)
    return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`
  }
  meetings[0].items[1] = {
    ...meetings[0].items[1],
    support: 'supported',
    status: 'commitment',
    due: day(0),
    recipient: '陈雪',
  }
  meetings[0].items[2] = {
    ...meetings[0].items[2],
    support: 'supported',
    status: 'commitment',
    due: day(-1),
  }
  meetings[0].segments[0].text = `李明负责更新客户报价，在 ${day(0)} 之前交付给陈雪。预算五十万。`
  meetings[0].segments[1].text = `陈雪明确承诺在 ${day(-1)} 之前将验收条件交付给李明。`
  meetings[0].segments.push({
    id: 's3',
    startMs: 10600000,
    endMs: 10630000,
    text: `李明明负责核对合同，在 ${day(5)} 前完成。项目需要安排另一个人确认报价，负责人未定。`,
    speaker: '李明明',
    speakerId: 'p3',
  })
  meetings[0].items.push(
    {
      kind: 'todo',
      text: '核对合同',
      owner: '李明明',
      due: day(5),
      recipient: null,
      evidence: ['s3'],
      done: false,
      support: 'supported',
      status: 'commitment',
    },
    {
      kind: 'todo',
      text: '安排报价确认',
      owner: null,
      due: '周五前',
      recipient: null,
      evidence: ['s3'],
      done: false,
      support: 'review',
      status: 'commitment',
    },
  )
}
const reviewPreview =
  typeof window !== 'undefined' &&
  new URLSearchParams(window.location.search).get('meeting-review') === '1'
if (reviewPreview) {
  meetings[0].items.forEach((item) => {
    item.support = 'review'
  })
}
if (
  reviewPreview &&
  new URLSearchParams(window.location.search).get('meeting-evidence') === 'missing'
) {
  meetings[0].items[0].evidence = ['missing']
}
let shortcutEnabled = false
let reply: {
  id: string
  phase: string
  original: string
  text: string
  error: string | null
  noteMeetingId?: string | null
} | null = null
let active: MeetingList['active'] = {
  id: 'processing',
  meetingId: meetings[0].id,
  kind: 'processing',
  paused: false,
  elapsedMs: 0,
  levels: [0, 0],
}
if (reviewPreview || followupPreview) active = null
let preferences = {
  myName: '李明',
  defaultTemplate: 'review',
  terms: [{ heard: '飞数', replacement: '飞书' }],
  captionTranslationEnabled: true,
}
const view: CaptionView = {
  uiLocale: 'zh-CN',
  sessionId: '22222222-2222-4222-8222-222222222222',
  meetingId: null,
  running: true,
  paused: false,
  mode: 'listen',
  targetLanguage: 'zh-CN',
  error: null,
  save: false,
  opacity: 0.92,
  fontSize: 24,
  original: true,
  passthrough: false,
  translationProviderId: null,
  translationEnabled: true,
  translationStatus: 'retrying',
  translationError: '服务超时。原文字幕继续显示，将重试翻译。',
  displayHeld: false,
  heldLines: [],
  lines: [
    {
      id: 'a',
      startMs: 1000,
      endMs: 14000,
      original: '我们已经讨论了客户需求，报价需要在周五前更新。',
      translated: '',
      finalText: true,
      source: 'system',
    },
    {
      id: 'b',
      startMs: 14000,
      endMs: 28000,
      original: '预算是五十万，超过这个数字需要重新审批。',
      translated: '',
      finalText: true,
      source: 'system',
    },
    {
      id: 'c',
      startMs: 28000,
      endMs: 35000,
      original: '刚才的决定是下周发布，验收条件由陈雪补充。',
      translated: '',
      finalText: true,
      source: 'system',
    },
  ],
}
function captionChanged() {
  window.dispatchEvent(new Event('captions_preview_changed'))
}
export async function experiencePreview(
  command: string,
  args: Record<string, unknown>,
): Promise<unknown> {
  if (
    ['meeting_start', 'meeting_resume'].includes(command) &&
    args.source !== 'system' &&
    typeof window !== 'undefined' &&
    new URLSearchParams(window.location.search).get('meeting-microphone') === 'denied'
  ) {
    throw { code: 'MicrophonePermission', userMessage: '请在设置中授权麦克风。' }
  }
  const meeting = meetings.find((m) => m.id === args.id || m.id === args.meetingId)
  switch (command) {
    case 'meeting_recording_view':
      if (
        recordingPreview &&
        new URLSearchParams(window.location.search).get('meeting-hud-locale') === 'en'
      )
        recordingPreview.uiLocale = 'en'
      return recordingPreview
    case 'meeting_recording_control':
      if (recordingPreview && args.id === recordingPreview.meetingId) {
        if (args.action === 'pause') recordingPreview.paused = true
        if (args.action === 'resume') recordingPreview.paused = false
        if (args.action === 'stop') recordingPreview = null
        window.dispatchEvent(new Event('meeting_hud_preview_changed'))
      }
      return
    case 'meeting_recording_window':
      return
    case 'meeting_recording_note': {
      if (!recordingPreview || recordingPreview.stopping || args.id !== recordingPreview.meetingId)
        throw new Error('这场会议录音已结束，请打开会议查看。')
      const failure = new URLSearchParams(window.location.search).get('meeting-note-error')
      if (failure === '1') throw new Error('会议文件读写失败，请检查剩余空间和文件权限。')
      const text = String(args.text || '').trim()
      const atMs = Number(args.atMs)
      if (atMs > recordingPreview.elapsedMs)
        throw new Error('标记时间超出当前录音，请重新打开便签。')
      if (meeting) {
        meeting.notes ??= []
        meeting.notes.push({ id: crypto.randomUUID(), atMs, text, kind: String(args.kind) })
        meeting.summaryStale = true
        meeting.revision++
      }
      return atMs
    }
    case 'meeting_prepare':
      return meetings
        .filter((m) => m.project === args.project)
        .sort((a, b) => b.createdAt - a.createdAt)
        .slice(0, 3)
    case 'experience_shortcuts_view':
      return { enabled: shortcutEnabled, error: null }
    case 'experience_shortcuts':
      shortcutEnabled = Boolean(args.enabled)
      return { enabled: shortcutEnabled, error: null }
    case 'meeting_item_verify':
      if (meeting) {
        if (meeting.revision !== args.revision) throw new Error('记录已更新，请重试。')
        const item = meeting.items[Number(args.index)]
        item.support = args.verified === false ? 'review' : 'supported'
        item.reviewed = args.verified !== false
        meeting.revision++
      }
      return
    case 'meeting_note_add':
      if (meeting) {
        meeting.notes ??= []
        meeting.notes.push({
          id: crypto.randomUUID(),
          atMs: meeting.durationMs,
          text: String(args.text || ''),
          kind: String(args.kind),
        })
        meeting.summaryStale = true
        meeting.revision++
      }
      return
    case 'meeting_note_update':
      if (meeting) {
        const note = meeting.notes?.find((n) => n.id === args.noteId)
        if (note) note.text = String(args.text)
        meeting.revision++
      }
      return
    case 'meeting_note_remove':
      if (meeting) {
        meeting.notes = meeting.notes?.filter((n) => n.id !== args.noteId)
        meeting.revision++
      }
      return
    case 'meeting_context_save':
      if (meeting) {
        if (meeting.revision !== args.revision) throw new Error('记录已更新，请重试。')
        meeting.project = String(args.project)
        meeting.templatePrompt = String(args.prompt)
        meeting.revision++
      }
      return
    case 'meeting_capture_image':
      return false
    case 'meeting_tasks_export':
      return false
    case 'meeting_update':
      if (meeting) {
        if (meeting.revision !== args.revision) throw new Error('记录已更新，请重试。')
        meeting.title = String(args.title)
        const oldItems = meeting.items
        meeting.items = structuredClone(args.items) as Meeting['items']
        meeting.items.forEach((item, index) => {
          const old = oldItems[index]
          if (
            ['text', 'kind', 'status', 'owner', 'due', 'recipient'].some(
              (key) => old?.[key as keyof typeof old] !== item[key as keyof typeof item],
            )
          ) {
            item.edited = true
            item.reviewed = false
            item.support = 'review'
          }
        })
        meeting.revision++
      }
      return
    case 'meeting_transcript_update':
      if (meeting) {
        const segments = structuredClone(args.segments) as Meeting['segments']
        meeting.dirtySegments = segments
          .filter((s) => meeting.segments.find((old) => old.id === s.id)?.text !== s.text)
          .map((s) => s.id)
        meeting.segments = segments
        meeting.items.forEach((item) => {
          if (item.evidence.some((id) => meeting.dirtySegments?.includes(id))) {
            item.support = 'review'
            item.reviewed = false
          }
        })
        meeting.summaryStale = true
        meeting.revision++
      }
      return
    case 'meeting_regenerate_affected':
      if (meeting) {
        meeting.dirtySegments = []
        meeting.summaryStale = false
        meeting.revision++
      }
      return
    case 'meeting_insight':
    case 'meeting_research': {
      const selected = meeting || meetings[0]
      const sourceMeetings =
        args.kind === 'changes' || command === 'meeting_research'
          ? [previous, selected]
          : [selected]
      const sources = sourceMeetings.flatMap((m) =>
        m.segments.map((s) => ({
          meetingId: m.id,
          title: m.title,
          createdAt: m.createdAt,
          revision: m.revision,
          segmentId: s.id,
          startMs: s.startMs,
          text: s.text,
        })),
      )
      const value = {
        kind: String(args.kind || 'research'),
        at: Date.now(),
        generated: false,
        sources,
        text:
          args.kind === 'closing'
            ? '已记录的待办中：0 项缺少负责人；2 项截止日期需要确认成具体日期。\n基于目前已整理的内容，尚未覆盖整场会议。'
            : args.kind === 'changes'
              ? '上次：周五上线，预算十五万。[1]\n本次：下周发布，预算五十万。[2]\n原文没有说明变化原因，需要进一步确认。'
              : args.kind === 'deliverable'
                ? '方案结论：下周发布。[1]\n预算：五十万；超过需重新审批。[1]\n验收条件：由陈雪补充，尚未确定具体条目。[2]'
                : '上次预算十五万。[1] 本次预算五十万。[2] 原文没有说明变化原因，仍需确认。',
      }
      if (meeting && args.kind !== 'closing') {
        meeting.artifacts ??= []
        meeting.artifacts = meeting.artifacts.filter((v) => v.kind !== value.kind)
        meeting.artifacts.push(value)
        meeting.revision++
      }
      return value
    }
    case 'captions_insight':
      return {
        generated: false,
        lines: view.lines,
        text:
          args.kind === 'reply'
            ? '建议回复：我需要先确认报价，再给你答复。\n可以追问：这次报价是否包含税费？'
            : args.kind === 'term'
              ? '这里的报价指本次客户方案的价格，具体是否含税仍需确认。'
              : '刚才明确提出需要更新报价，并补充验收条件。请结合完整上下文核对。',
      }
    case 'captions_panel_size':
      return
    case 'captions_reply_view':
      return reply
    case 'captions_reply_start':
      reply = {
        id: crypto.randomUUID(),
        noteMeetingId: args.noteMeetingId ? String(args.noteMeetingId) : null,
        phase: 'recording',
        original: '我需要确认一下',
        text: '',
        error: null,
      }
      window.dispatchEvent(new Event('caption_reply_preview_changed'))
      return reply.id
    case 'captions_reply_control':
      if (reply && reply.id === args.id) {
        reply.phase = args.action === 'cancel' ? 'cancelled' : 'ready'
        reply.text = reply.noteMeetingId ? reply.original : 'I need to confirm this first.'
        if (reply.noteMeetingId && args.action !== 'cancel') {
          const target = meetings.find((m) => m.id === reply?.noteMeetingId)
          if (target) {
            target.notes ??= []
            target.notes.push({
              id: crypto.randomUUID(),
              atMs: target.durationMs,
              text: reply.original,
              kind: 'supplement',
            })
            target.revision++
            target.summaryStale = true
          }
        }
        window.dispatchEvent(new Event('caption_reply_preview_changed'))
      }
      return
    case 'meeting_audio': {
      if (new URLSearchParams(window.location.search).get('meeting-audio') === 'missing')
        throw {
          code: 'MeetingAudioMissing',
          userMessage: '本段录音文件缺失或损坏，仍可查看逐字稿。',
        }
      // One second of silent PCM for player QA; never uses a microphone or user audio.
      const bytes = new Uint8Array(44 + 16000 * 2)
      const data = new DataView(bytes.buffer)
      for (const [offset, text] of [
        [0, 'RIFF'],
        [8, 'WAVE'],
        [12, 'fmt '],
        [36, 'data'],
      ] as const)
        for (let i = 0; i < text.length; i++) bytes[offset + i] = text.charCodeAt(i)
      data.setUint32(4, bytes.length - 8, true)
      data.setUint32(16, 16, true)
      data.setUint16(20, 1, true)
      data.setUint16(22, 1, true)
      data.setUint32(24, 16000, true)
      data.setUint32(28, 32000, true)
      data.setUint16(32, 2, true)
      data.setUint16(34, 16, true)
      data.setUint32(40, bytes.length - 44, true)
      return Array.from(bytes)
    }
    case 'meeting_list':
      return {
        meetings,
        active,
        textModelReady: true,
        textModelBytes: 0,
        downloadedBytes: 0,
        downloadError: null,
        downloadModel: null,
      }
    case 'meeting_get':
      if (!meeting) throw new Error('会议不存在')
      return structuredClone(meeting)
    case 'meeting_preferences':
      return structuredClone(preferences)
    case 'meeting_preferences_save':
      preferences = structuredClone(args.preferences) as typeof preferences
      return
    case 'meeting_tasks':
      return meetings.flatMap((m) =>
        m.items.flatMap((item, index) =>
          item.kind === 'todo'
            ? [
                {
                  meetingId: m.id,
                  title: m.title,
                  project: m.project,
                  createdAt: m.createdAt,
                  state: m.state,
                  summaryStale: m.summaryStale,
                  revision: m.revision,
                  index,
                  item,
                },
              ]
            : [],
        ),
      )
    case 'meeting_task_update':
      if (meeting) {
        if (meeting.revision !== args.revision) throw new Error('记录已更新，请重试。')
        const item = meeting.items[Number(args.index)]
        const edit = args.edit as {
          text: string
          owner: string | null
          due: string | null
          recipient: string | null
          confirmed: boolean
        }
        const changed = ['text', 'owner', 'due', 'recipient'].some(
          (key) => item[key as keyof typeof item] !== edit[key as keyof typeof edit],
        )
        Object.assign(item, {
          text: edit.text.trim(),
          owner: edit.owner,
          due: edit.due,
          recipient: edit.recipient,
        })
        if (changed) {
          item.edited = true
          item.reviewed = false
          item.support = 'review'
        }
        if (edit.confirmed) {
          item.reviewed = true
          item.support = 'supported'
        }
        meeting.revision++
      }
      return
    case 'meeting_task_toggle':
      if (meeting) {
        if (meeting.revision !== args.revision) throw new Error('记录已更新，请重试。')
        meeting.items[Number(args.index)].done = Boolean(args.done)
        meeting.revision++
      }
      return
    case 'meeting_search':
      return meetings.flatMap((m) =>
        m.segments
          .filter(
            (s) =>
              `${s.text} ${s.speaker} ${m.title}`.includes(String(args.query)) &&
              (!args.project ||
                (m.project || '')
                  .toLowerCase()
                  .includes(String(args.project).trim().toLowerCase())) &&
              (!args.after || m.createdAt >= Number(args.after)) &&
              (!args.before || m.createdAt <= Number(args.before)) &&
              (!args.speaker || String(s.speaker).includes(String(args.speaker))),
          )
          .map((s) => ({
            meetingId: m.id,
            title: m.title,
            createdAt: m.createdAt,
            snippet: s.text,
            segmentId: s.id,
            startMs: s.startMs,
          })),
      )
    case 'meeting_start': {
      const m = structuredClone(meetings[0])
      m.id = '33333333-3333-4333-8333-333333333333'
      m.title = String(args.title || '下一场会议')
      m.source = String(args.source)
      m.template = String(args.template)
      m.project = String(args.project || '')
      m.notes = []
      m.artifacts = []
      m.dirtySegments = []
      m.state = 'recording'
      m.items = []
      m.segments = []
      m.gaps = []
      m.durationMs = 0
      m.revision = 1
      m.processedSamples = 0
      meetings.push(m)
      active = {
        id: m.id,
        meetingId: m.id,
        kind: 'recording',
        paused: false,
        elapsedMs: 0,
        levels: [0.02, 0.05],
      }
      return m.id
    }
    case 'meeting_resume':
      if (meeting) {
        meeting.gaps.push({
          afterMs: meeting.durationMs,
          startedAt: Date.now() - 60000,
          endedAt: Date.now(),
          reason: '继续同一场会议录音',
        })
        meeting.state = 'recording'
        meeting.revision++
        active = {
          id: meeting.id,
          meetingId: meeting.id,
          kind: 'recording',
          paused: false,
          elapsedMs: meeting.durationMs,
          levels: [0.02, 0.05],
        }
      }
      return
    case 'meeting_control':
      if (active && meeting) {
        if (args.action === 'stop' || args.action === 'cancel') {
          meeting.state = 'queued'
          active = null
        } else active.paused = args.action === 'pause'
        meeting.revision++
      }
      return
    case 'meeting_template_choose':
      if (meeting) {
        meeting.template = String(args.template)
        meeting.summaryStale = true
        meeting.revision++
      }
      return
    case 'meeting_speaker_rename':
      if (meeting) {
        const target = String(args.mergeWith || args.speakerId)
        meeting.segments.forEach((s) => {
          if (s.speakerId === args.speakerId || s.speakerId === target) {
            s.speaker = String(args.name)
            s.speakerId = target
          }
        })
        meeting.revision++
      }
      return
    case 'meeting_process':
      if (meeting) {
        meeting.state = 'summarizing'
        meeting.revision++
        active = {
          id: meeting.id,
          meetingId: meeting.id,
          kind: 'processing',
          paused: false,
          elapsedMs: 0,
          levels: [0, 0],
        }
      }
      return
    case 'meeting_models':
      return {
        models: [
          { id: 'moss', name: 'MOSS', size: 0, ready: true },
          { id: 'speaker', name: 'CAMPPlus', size: 0, ready: true },
        ],
        active: null,
        busy: false,
        downloaded: 0,
        error: null,
        downloadModel: null,
        downloadPhase: null,
        mossRuntime: { ready: true, downloadBytes: 0, path: '' },
      }
    case 'meeting_versions':
    case 'captions_archives':
      return []
    case 'captions_view':
      return structuredClone(view)
    case 'captions_appearance':
      Object.assign(view, {
        fontSize: args.fontSize,
        opacity: args.opacity,
        original: args.original,
        passthrough: args.passthrough,
      })
      captionChanged()
      return
    case 'captions_translation':
      view.translationEnabled = Boolean(args.enabled)
      view.translationStatus = view.translationEnabled ? 'active' : 'off'
      view.translationError = null
      captionChanged()
      return
    case 'captions_control':
      if (args.action === 'hold') {
        view.displayHeld = true
        view.heldLines = structuredClone(view.lines)
      } else if (args.action === 'live') {
        view.displayHeld = false
        view.heldLines = []
      } else if (args.action === 'stop') view.running = false
      else view.paused = args.action === 'pause'
      captionChanged()
      return
    case 'captions_recap':
      return {
        text: view.lines.map((l) => l.original).join('\n'),
        lines: view.lines,
        generated: false,
      }
    case 'captions_copy':
      return
    case 'captions_window':
      return
    case 'meeting_summary_settings':
      return { providerId: null, followAi: false }
    case 'meeting_summary_choose':
      return
    case 'assistant_view':
      return {
        sessionId: 'preview',
        messages: [],
        busy: false,
        phase: null,
        hasMemory: false,
        persistent: false,
        sourceCount: 3,
      }
    case 'meeting_notion_status':
      return { connected: false }
    default:
      throw new Error('此操作不在测试样例范围内。')
  }
}
