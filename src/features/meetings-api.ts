import type { Translate } from '@/lib/errors'
import { invoke, isTauri } from '@tauri-apps/api/core'

export interface MeetingItem {
  kind: 'decision' | 'todo' | 'fact' | 'next'
  text: string
  owner: string | null
  due: string | null
  evidence: string[]
  done: boolean
  status?: 'proposal' | 'decision' | 'commitment' | 'pending' | 'information' | ''
  recipient?: string | null
  edited?: boolean
  reviewed?: boolean
  support?: 'supported' | 'review' | ''
}
export interface Insight {
  kind: string
  text: string
  generated: boolean
  stale?: boolean
  at: number
  sources: {
    meetingId: string
    title: string
    createdAt: number
    revision: number
    segmentId: string
    startMs: number
    text: string
  }[]
}
export const itemStatus = (item: MeetingItem) =>
  item.status ||
  ({ decision: 'decision', todo: 'commitment', fact: 'information', next: 'proposal' } as const)[
    item.kind
  ]
export interface MeetingSegment {
  speakerId?: string | null
  speaker: string | null
  id: string
  startMs: number
  endMs: number
  text: string
}
export interface Meeting {
  template: string
  participants?: string[]
  project?: string
  templatePrompt?: string
  dirtySegments?: string[]
  artifacts?: Insight[]
  notes?: { id: string; atMs: number; text: string; kind: string; image?: string | null }[]
  systemApp?: { bundleId: string; name: string } | null
  gaps: { afterMs: number; startedAt: number; endedAt: number | null; reason: string }[]
  id: string
  title: string
  createdAt: number
  durationMs: number
  source: string
  state: string
  error: string | null
  revision: number
  language: string
  segments: MeetingSegment[]
  items: MeetingItem[]
  audioDeleted: boolean
  retentionDays: number | null
  summaryStale: boolean
  summaryProviderId: string | null
  summaryFollowAi: boolean
  processedSamples: number
  modelId: string | null
}
export interface MeetingList {
  meetings: Meeting[]
  active: {
    id: string
    meetingId: string | null
    kind: string
    paused: boolean
    elapsedMs: number
    levels: number[]
  } | null
  textModelReady: boolean
  textModelBytes: number
  downloadedBytes: number
  downloadError: string | null
  downloadModel: string | null
}
export interface ImportChoice {
  choiceId: string
  name: string
  durationSeconds: number
}
export interface MeetingModels {
  models: { id: string; name: string; size: number; ready: boolean }[]
  active: string | null
  busy: boolean
  downloaded: number
  error: string | null
  downloadModel: string | null
  downloadPhase: 'downloading_runtime' | 'installing_runtime' | 'downloading_model' | null
  mossRuntime: { ready: boolean; downloadBytes: number; path: string }
}
export function meetingDownloadLabel(data: MeetingModels, t: Translate) {
  if (data.downloadPhase === 'installing_runtime')
    return t('正在安装并检查 MOSS 运行环境', 'Installing and checking the MOSS runtime')
  const label =
    data.downloadPhase === 'downloading_runtime'
      ? t('正在下载 MOSS 运行环境', 'Downloading the MOSS runtime')
      : t('正在下载模型', 'Downloading model')
  return `${label} · ${(data.downloaded / 1e6).toFixed(0)} MB`
}
export async function meetingCall<T>(
  command: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  if (isTauri()) return invoke<T>(command, args)
  if (
    import.meta.env.DEV &&
    typeof window !== 'undefined' &&
    new URLSearchParams(window.location.search).get('meeting-fixtures') === '1'
  ) {
    return (await import('./meeting-experience.preview')).experiencePreview(
      command,
      args,
    ) as Promise<T>
  }
  if (command === 'experience_shortcuts_view') return { enabled: false, error: null } as T
  if (command === 'captions_reply_view') return null as T
  if (command === 'meeting_recording_view') return null as T
  if (command === 'meeting_preferences')
    return { myName: '', defaultTemplate: 'general', terms: [] } as T
  if (command === 'meeting_prepare') return [] as T
  if (command === 'meeting_tasks' || command === 'meeting_search') return [] as T
  if (command === 'assistant_view')
    return {
      sessionId: 'preview',
      messages: [],
      busy: false,
      phase: null,
      hasMemory: false,
      persistent: true,
      sourceCount: 0,
    } as T
  if (command === 'meeting_summary_settings') return { providerId: null, followAi: true } as T
  if (command === 'meeting_models')
    return {
      models: [],
      active: null,
      busy: false,
      downloaded: 0,
      error: null,
      downloadModel: null,
      downloadPhase: null,
      mossRuntime: { ready: false, downloadBytes: 278600000, path: '' },
    } as T
  if (command === 'captions_view')
    return {
      translationEnabled: false,
      translationStatus: 'idle',
      translationError: null,
      displayHeld: false,
      heldLines: [],
      running: false,
      paused: false,
      mode: 'listen',
      targetLanguage: 'zh-CN',
      error: null,
      lines: [],
      save: false,
      opacity: 0.92,
      fontSize: 24,
      original: true,
      passthrough: false,
    } as T
  if (command === 'meeting_list')
    return {
      meetings: [],
      active: null,
      textModelReady: false,
      textModelBytes: 2497280256,
      downloadedBytes: 0,
      downloadError: null,
      downloadModel: null,
    } as T
  throw new Error('请在 SpeakMore 桌面应用中使用会议功能。')
}
export const meetingStatus = (t: Translate): Record<string, string> => ({
  preparing: t('正在准备', 'Preparing'),
  recording: t('录音中', 'Recording'),
  paused: t('已暂停', 'Paused'),
  importing: t('正在导入', 'Importing'),
  saved: t('录音已保存', 'Recording saved'),
  queued: t('等待继续整理', 'Waiting to resume'),
  interrupted: t('可恢复整理', 'Ready to resume'),
  transcribing: t('正在转写', 'Transcribing'),
  summarizing: t('正在整理', 'Summarizing'),
  waiting_model: t('逐字稿已完成', 'Transcript ready'),
  ready: t('纪要已生成', 'Notes ready'),
  failed: t('需要处理', 'Needs attention'),
})
export function meetingStatusLabel(
  meeting: Pick<Meeting, 'state' | 'source' | 'durationMs' | 'processedSamples' | 'segments'>,
  t: Translate,
) {
  if (
    meeting.state === 'failed' &&
    meeting.source !== 'import' &&
    meeting.durationMs === 0 &&
    meeting.processedSamples === 0 &&
    meeting.segments.length === 0
  )
    return t('未开始录音', 'Recording did not start')
  return meetingStatus(t)[meeting.state] ?? meeting.state
}
export function timecode(ms: number) {
  return `${Math.floor(ms / 60000)
    .toString()
    .padStart(2, '0')}:${Math.floor((ms / 1000) % 60)
    .toString()
    .padStart(2, '0')}`
}
export function meetingError(e: unknown) {
  if (e && typeof e === 'object' && 'userMessage' in e) return String(e.userMessage)
  return e instanceof Error ? e.message : '操作未完成，请重试。'
}
export const meetingChangedMessage = '会议内容已更新，请关闭此窗口后重新打开再修改。'
export const meetingMissingMessage = '这场会议已不存在，可能已被删除。'
// For edits submitted with the revision seen when a dialog opened: a conflict means the meeting changed meanwhile.
export function meetingEditError(e: unknown) {
  if (e && typeof e === 'object' && 'code' in e && e.code === 'RevisionConflict')
    return meetingChangedMessage
  return meetingError(e)
}
// Polling identities: when they match, keep the previous object so the page does not re-render.
// Elapsed time and audio levels are left out; the live recording time has its own fast source.
export function meetingListKey(list: MeetingList) {
  const a = list.active
  return JSON.stringify([
    list.meetings.map((m) => `${m.id}:${m.revision}`),
    a && [a.id, a.meetingId, a.kind, a.paused],
    list.textModelReady,
    list.textModelBytes,
    list.downloadedBytes,
    list.downloadError,
    list.downloadModel,
  ])
}
export function meetingKey(m: Meeting) {
  // Result-card staleness depends on other meetings, so it can change without a revision bump.
  return `${m.id}:${m.revision}:${(m.artifacts ?? []).map((a) => (a.stale ? 1 : 0)).join('')}`
}
