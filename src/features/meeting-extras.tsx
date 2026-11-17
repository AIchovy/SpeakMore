import { meetingMessage } from './meeting-messages'
import { useText } from '@/lib/app-state'
import { Checkbox } from '@/components/ui/checkbox'
import { MossSetup } from './moss-setup'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from '@/components/ui/dialog'
import {
  PiClockCounterClockwise,
  PiPencilSimple,
  PiPaperPlaneTilt,
  PiGearSix,
} from 'react-icons/pi'
import {
  meetingCall,
  meetingEditError,
  meetingError,
  meetingDownloadLabel,
  timecode,
  type Meeting,
  type MeetingItem,
  type MeetingModels,
} from './meetings-api'
const field = 'w-full rounded-md border bg-background px-3 py-2 text-sm'
type Version = { revision: number; items: MeetingItem[]; segments: Meeting['segments'] }
export function MeetingExtras({
  meeting,
  disabled,
  refresh,
}: {
  meeting: Meeting
  disabled: boolean
  refresh: () => Promise<void>
}) {
  const t = useText()

  const [panel, setPanel] = useState<string | null>(null),
    [draft, setDraft] = useState(meeting.segments),
    [versions, setVersions] = useState<Version[]>([]),
    [version, setVersion] = useState<Version | null>(null),
    // The revision a transcript or version dialog was opened at; saving after a newer change
    // is rejected as a conflict instead of overwriting it.
    [openedRevision, setOpenedRevision] = useState(meeting.revision)
  const [error, setError] = useState(''),
    [busy, setBusy] = useState(false),
    [parent, setParent] = useState(''),
    [token, setToken] = useState(''),
    [connected, setConnected] = useState(false),
    [include, setInclude] = useState(false),
    [status, setStatus] = useState<{ state: string; url?: string; revision?: number }>({
      state: '',
    })
  const act = async (fn: () => Promise<void>, describe = meetingError) => {
    setBusy(true)
    setError('')
    try {
      await fn()
      await refresh()
    } catch (e) {
      setError(describe(e))
    } finally {
      setBusy(false)
    }
  }
  const open = (name: string) =>
    void act(async () => {
      setPanel(name)
      setOpenedRevision(meeting.revision)
      if (name === 'transcript') setDraft(meeting.segments.map((s) => ({ ...s })))
      if (name === 'versions') {
        setVersions(await meetingCall('meeting_versions', { id: meeting.id }))
        setVersion(null)
      }
      if (name === 'notion') {
        const c = await meetingCall<{ parentId: string; connected: boolean }>(
          'meeting_notion_connection',
        )
        setConnected(c.connected)
        setParent(c.parentId)
        setStatus(await meetingCall('meeting_notion_status', { id: meeting.id }))
      }
    })
  return (
    <>
      <div className="mb-6 flex flex-wrap items-center gap-2">
        <Button
          size="sm"
          variant="ghost"
          disabled={disabled || busy || !meeting.segments.length}
          onClick={() => open('transcript')}
        >
          <PiPencilSimple />
          {t('编辑原文与说话人', 'Edit transcript and speakers')}
        </Button>
        <Button
          size="sm"
          variant="ghost"
          disabled={disabled || busy}
          onClick={() => open('versions')}
        >
          <PiClockCounterClockwise />
          {t('版本', 'Versions')}
        </Button>
        <Button
          size="sm"
          variant="ghost"
          disabled={disabled || busy || !meeting.items.length}
          onClick={() => open('notion')}
        >
          <PiPaperPlaneTilt />
          {t('发送到 Notion', 'Send to Notion')}
        </Button>
        <Button
          size="sm"
          variant="ghost"
          disabled={disabled || busy}
          onClick={() => open('retention')}
        >
          <PiGearSix />
          {t('音频保留', 'Audio retention')}
        </Button>
      </div>
      {!disabled && !meeting.audioDeleted && (
        <details className="mb-5 text-xs text-muted-foreground">
          <summary className="cursor-pointer">
            {t('更换转写模型', 'Change transcription model')}
          </summary>
          <p className="my-3">
            {t(
              '切换后需要重新转写，当前内容会保存到历史版本。',
              'Changing the model requires a new transcription. Current content is kept in version history.',
            )}
          </p>
          <div className="flex gap-2">
            {[
              ['', t('当前语音模型', 'Current speech model')],
              ['moss', t('MOSS · 说话人', 'MOSS · Speakers')],
            ].map(([id, label]) => (
              <Button
                key={id}
                variant="outline"
                size="sm"
                disabled={busy}
                onClick={() =>
                  void act(async () => {
                    await meetingCall('meeting_model_choose', {
                      id: meeting.id,
                      revision: meeting.revision,
                      model: id || null,
                    })
                  })
                }
              >
                {label}
              </Button>
            ))}
          </div>
        </details>
      )}
      {meeting.summaryStale && (
        <p className="mb-5 rounded-lg bg-muted px-4 py-3 text-sm">
          {t(
            '内容或整理要求已更新。现有纪要已保留，可重新整理生成新版本。',
            'Content or preferences have changed. Existing notes are kept; regenerate to create a new version.',
          )}
        </p>
      )}
      {!disabled && meeting.modelId === 'moss' && !meeting.audioDeleted && (
        <div className="mb-5">
          <MossSetup />
        </div>
      )}
      {meeting.audioDeleted && (
        <p className="mb-5 text-xs text-muted-foreground">
          {t(
            '音频已按保留期清理，逐字稿和纪要仍可使用。',
            'Audio was removed according to your retention settings. The transcript and notes remain available.',
          )}
        </p>
      )}
      <Dialog
        open={!!panel}
        onOpenChange={(o) => {
          if (!o && !busy) {
            setPanel(null)
            setError('')
          }
        }}
      >
        <DialogContent
          closeLabel={t('关闭', 'Close')}
          className={
            panel === 'transcript' || panel === 'versions'
              ? 'sm:max-w-[900px] max-h-[85vh] overflow-y-auto'
              : ''
          }
        >
          <DialogHeader>
            <DialogTitle>
              {
                {
                  transcript: t('编辑原文与说话人', 'Edit transcript and speakers'),
                  versions: t('纪要版本', 'Note versions'),
                  notion: t('发送到 Notion', 'Send to Notion'),
                  retention: t('音频保留', 'Audio retention'),
                }[panel ?? '']
              }
            </DialogTitle>
            <DialogDescription>
              {panel === 'transcript'
                ? t(
                    '修改原文后可重新生成纪要。MOSS 会在本场会议内关联说话人，归属不确定时保留待确认标记。',
                    'Regenerate notes after editing the transcript. MOSS links speakers within this meeting and marks uncertain identities for review.',
                  )
                : panel === 'notion'
                  ? t(
                      '仅发送下方选择的文字；录音留在本机。',
                      'Only the text selected below is sent. Audio stays on your device.',
                    )
                  : panel === 'retention'
                    ? t(
                        '清理音频不会删除纪要。未完成整理的会议不会自动清理。',
                        'Removing audio keeps your notes. Unprocessed meetings are not automatically cleaned up.',
                      )
                    : t(
                        '每次重新整理或编辑原文前会保留旧版本。',
                        'A previous version is kept before regenerating notes or editing the transcript.',
                      )}
            </DialogDescription>
          </DialogHeader>
          {panel === 'transcript' && (
            <div className="space-y-5">
              {draft.map((s, i) => (
                <div className="grid grid-cols-[88px_1fr] gap-3" key={s.id}>
                  <span className="pt-2 text-xs text-muted-foreground tabular-nums">
                    {timecode(s.startMs)}
                  </span>
                  <div className="space-y-2">
                    <Input
                      aria-label={t(`说话人 ${i + 1}`, `Speaker ${i + 1}`)}
                      placeholder={t('说话人（可选）', 'Speaker (optional)')}
                      maxLength={80}
                      value={s.speaker ?? ''}
                      onChange={(e) =>
                        setDraft((a) =>
                          a.map((v, n) =>
                            n === i ? { ...v, speaker: e.target.value || null } : v,
                          ),
                        )
                      }
                    />
                    <textarea
                      aria-label={t(`原文 ${i + 1}`, `Transcript ${i + 1}`)}
                      className={field + ' min-h-24 leading-7'}
                      value={s.text}
                      maxLength={12000}
                      onChange={(e) =>
                        setDraft((a) =>
                          a.map((v, n) => (n === i ? { ...v, text: e.target.value } : v)),
                        )
                      }
                    />
                  </div>
                </div>
              ))}
            </div>
          )}
          {panel === 'versions' && (
            <>
              <Select
                value={version ? String(version.revision) : ''}
                onValueChange={(value) =>
                  setVersion(versions.find((v) => v.revision === Number(value)) ?? null)
                }
              >
                <SelectTrigger aria-label={t('历史版本', 'Version history')} className="w-full">
                  <SelectValue placeholder={t('选择历史版本', 'Choose a version')} />
                </SelectTrigger>
                <SelectContent>
                  {versions.map((v) => (
                    <SelectItem key={v.revision} value={String(v.revision)}>
                      {t('版本', 'Versions')}
                      {v.revision} · {v.items.length} {t('条事项', 'items')}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {!versions.length && (
                <p className="py-6 text-sm text-muted-foreground">
                  {t('暂无历史版本。', 'No previous versions yet.')}
                </p>
              )}
              {version && (
                <div className="grid gap-6 sm:grid-cols-2">
                  {[
                    [t('历史版本', 'Version history'), version.items],
                    [t('当前版本', 'Current version'), meeting.items],
                  ].map(([name, items]) => (
                    <section key={name as string}>
                      <h3 className="mb-3 text-sm font-medium">{name as string}</h3>
                      <div className="space-y-3">
                        {(items as MeetingItem[]).map((i, n) => (
                          <p key={n} className="text-sm leading-7">
                            {i.text}
                            {i.owner && ` · ${i.owner}`}
                            {i.due && ` · ${i.due}`}
                          </p>
                        ))}
                      </div>
                    </section>
                  ))}
                </div>
              )}
            </>
          )}
          {panel === 'notion' && (
            <div className="space-y-4">
              {!connected ? (
                <>
                  <p className="text-sm leading-6 text-muted-foreground">
                    {t(
                      '创建 Notion 内部连接，并将目标父页面共享给该连接。密钥保存在 macOS 钥匙串中。',
                      'Create a Notion internal integration and share the parent page with it. Your secret is stored in macOS Keychain.',
                    )}
                  </p>
                  <Input
                    aria-label={t('父页面 ID', 'Parent page ID')}
                    placeholder={t('父页面 ID', 'Parent page ID')}
                    value={parent}
                    onChange={(e) => setParent(e.target.value)}
                  />
                  <Input
                    aria-label={t('Notion 连接密钥', 'Notion integration secret')}
                    type="password"
                    autoComplete="off"
                    placeholder={t('Notion 连接密钥', 'Notion integration secret')}
                    value={token}
                    onChange={(e) => setToken(e.target.value)}
                  />
                  <Button
                    disabled={busy}
                    onClick={() =>
                      void act(async () => {
                        await meetingCall('meeting_notion_save', { parentId: parent, token })
                        setToken('')
                        setConnected(true)
                      })
                    }
                  >
                    {t('保存连接', 'Save connection')}
                  </Button>
                </>
              ) : (
                <>
                  <div className="flex justify-between items-center text-sm">
                    <span>{t('已连接 Notion', 'Connected to Notion')}</span>
                    <Button
                      variant="ghost"
                      size="sm"
                      disabled={busy}
                      onClick={() =>
                        void act(async () => {
                          await meetingCall('meeting_notion_disconnect')
                          setConnected(false)
                        })
                      }
                    >
                      {t('断开', 'Disconnect')}
                    </Button>
                  </div>
                  <div className="rounded-lg bg-muted p-4">
                    <p className="font-medium text-sm">{meeting.title}</p>
                    <p className="mt-2 text-xs text-muted-foreground">
                      {t(
                        '决策、待办、关键数字与结论、下一步 ·',
                        'Decisions, action items, key findings and next steps ·',
                      )}
                      {meeting.items.length} {t('条', 'items')}
                    </p>
                  </div>
                  <label className="flex items-center gap-2 text-sm">
                    <Checkbox
                      checked={include}
                      onCheckedChange={(value) => setInclude(value === true)}
                    />
                    {t('同时发送逐字稿', 'Include transcript')}
                  </label>
                  {status.state === 'complete' && (
                    <p className="text-sm">
                      {t(
                        '已发送到 Notion，可在目标父页面查看。',
                        'Sent to Notion. View it under the selected parent page.',
                      )}
                    </p>
                  )}
                  {status.state === 'pending' && (
                    <p className="text-sm">
                      {t(
                        '上次请求结果待核实。核实后会继续尚未完成的部分。',
                        'The previous request needs verification. Once verified, unfinished parts will resume.',
                      )}
                    </p>
                  )}
                </>
              )}
            </div>
          )}
          {panel === 'retention' && (
            <Select
              disabled={meeting.audioDeleted || busy}
              value={String(meeting.retentionDays ?? 'forever')}
              onValueChange={(value) =>
                void act(async () => {
                  await meetingCall('meeting_retention', {
                    id: meeting.id,
                    days: value === 'forever' ? null : Number(value),
                  })
                })
              }
            >
              <SelectTrigger
                aria-label={t('音频保留期限', 'Audio retention period')}
                className="w-full"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="0">{t('完成整理后清理', 'Remove after processing')}</SelectItem>
                <SelectItem value="7">{t('7 天', '7 days')}</SelectItem>
                <SelectItem value="30">{t('30 天', '30 days')}</SelectItem>
                <SelectItem value="forever">{t('永久保留', 'Keep forever')}</SelectItem>
              </SelectContent>
            </Select>
          )}
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {meetingMessage(error, t)}
            </p>
          )}
          <DialogFooter>
            <Button variant="outline" disabled={busy} onClick={() => setPanel(null)}>
              {t('关闭', 'Close')}
            </Button>
            {panel === 'transcript' && (
              <Button
                disabled={busy || disabled}
                onClick={() =>
                  void act(async () => {
                    await meetingCall('meeting_transcript_update', {
                      id: meeting.id,
                      revision: openedRevision,
                      segments: draft,
                    })
                    setPanel(null)
                  }, meetingEditError)
                }
              >
                {t('保存原文', 'Save transcript')}
              </Button>
            )}
            {panel === 'versions' && version && (
              <Button
                disabled={busy || disabled}
                onClick={() =>
                  void act(async () => {
                    await meetingCall('meeting_restore', {
                      id: meeting.id,
                      revision: openedRevision,
                      version: version.revision,
                    })
                    setPanel(null)
                  }, meetingEditError)
                }
              >
                {t('恢复此版本', 'Restore this version')}
              </Button>
            )}
            {panel === 'notion' &&
              connected &&
              (status.state !== 'complete' || status.revision !== meeting.revision) && (
                <Button
                  disabled={busy}
                  onClick={() =>
                    void act(async () => {
                      try {
                        setStatus(
                          await meetingCall('meeting_notion_send', {
                            id: meeting.id,
                            transcript: include,
                            verify: status.state === 'pending',
                          }),
                        )
                      } finally {
                        setStatus(await meetingCall('meeting_notion_status', { id: meeting.id }))
                      }
                    })
                  }
                >
                  {busy
                    ? t('正在发送…', 'Sending…')
                    : status.state === 'pending'
                      ? t('核实并继续', 'Verify and continue')
                      : status.state === 'complete'
                        ? t('发送更新版本', 'Send updated version')
                        : t('发送纪要', 'Send notes')}
                </Button>
              )}
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
export function AudioSource({
  source,
  onSource,
  pid,
  onPid,
}: {
  source: string
  onSource: (v: string) => void
  pid: number | null
  onPid: (v: number | null) => void
}) {
  const t = useText()

  const [apps, setApps] = useState<{ pid: number; name: string }[]>([]),
    [appPickerOpen, setAppPickerOpen] = useState(false),
    [loadingApps, setLoadingApps] = useState(false),
    [error, setError] = useState('')
  return (
    <div className="space-y-3">
      <label className="grid gap-3 text-sm">
        <span>{t('录音来源', 'Recording source')}</span>
        <Select value={source} onValueChange={onSource}>
          <SelectTrigger aria-label={t('录音来源', 'Recording source')} className="w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="microphone">{t('麦克风', 'Microphone')}</SelectItem>
            <SelectItem value="system">{t('系统声音', 'System audio')}</SelectItem>
            <SelectItem value="mixed">
              {t('系统声音与麦克风', 'System audio and microphone')}
            </SelectItem>
          </SelectContent>
        </Select>
      </label>
      {source !== 'microphone' && (
        <>
          <div className="flex gap-2">
            <Select
              value={String(pid ?? 0)}
              open={appPickerOpen}
              onOpenChange={setAppPickerOpen}
              onValueChange={(value) => onPid(Number(value) || null)}
            >
              <SelectTrigger aria-label={t('系统音源', 'System audio source')} className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="0">{t('全部系统声音', 'All system audio')}</SelectItem>
                {apps.map((a) => (
                  <SelectItem key={a.pid} value={String(a.pid)}>
                    {a.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Button
              variant="outline"
              disabled={loadingApps}
              onClick={() => {
                setError('')
                setLoadingApps(true)
                void meetingCall<typeof apps>('meeting_audio_apps')
                  .then((values) => {
                    setApps(values)
                    setAppPickerOpen(true)
                  })
                  .catch((e) => setError(meetingError(e)))
                  .finally(() => setLoadingApps(false))
              }}
            >
              {t('选择应用', 'Choose app')}
            </Button>
          </div>
          {source === 'mixed' && (
            <p className="text-xs leading-5 text-muted-foreground">
              {t(
                '建议佩戴耳机，避免扬声器声音被麦克风重复录下。',
                'Use headphones to avoid recording speaker audio through the microphone twice.',
              )}
            </p>
          )}
          {error && (
            <p role="alert" className="text-xs text-destructive">
              {meetingMessage(error, t)}
            </p>
          )}
        </>
      )}
    </div>
  )
}
export function ExtendedModels() {
  const t = useText()

  const [data, setData] = useState<MeetingModels | null>(null),
    [error, setError] = useState(''),
    // Polling failures clear themselves on the next successful read; action errors stay.
    [pollError, setPollError] = useState(''),
    [pending, setPending] = useState('')
  const read = () =>
    meetingCall<NonNullable<typeof data>>('meeting_models')
      .then((value) => {
        setData(value)
        setPollError('')
      })
      .catch((e) => setPollError(meetingError(e)))
  useEffect(() => {
    void read()
    const t = setInterval(() => void read(), 2000)
    return () => clearInterval(t)
  }, [])
  const act = async (cmd: string, id: string) => {
    setPending(id)
    setError('')
    try {
      await meetingCall(cmd, { id })
      await read()
    } catch (e) {
      setError(meetingError(e))
    } finally {
      setPending('')
    }
  }
  return (
    <section className="mt-9 border-t pt-7">
      <h2 className="text-base font-medium">
        {t('会议与实时字幕模型', 'Meeting and live caption models')}
      </h2>
      <div className="mt-4 divide-y">
        {data?.models.map((m) => (
          <div key={m.id} className="flex flex-wrap items-center justify-between gap-5 py-5">
            <div>
              <p className="text-sm font-medium">{meetingMessage(m.name, t)}</p>
              <p className="mt-2 text-xs text-muted-foreground">
                {m.size < 1e9
                  ? `${(m.size / 1e6).toFixed(1)} MB`
                  : `${(m.size / 1e9).toFixed(2)} GB`}{' '}
                ·{' '}
                {m.id === 'moss'
                  ? t(
                      'Apple GPU / CPU · 说话人区分 · 建议 16 GB 以上内存',
                      'Apple GPU / CPU · Speaker identification · 16 GB RAM or more recommended',
                    )
                  : m.id === 'speaker'
                    ? t(
                        '本场声音特征匹配 · 配合 MOSS 连续识别说话人 · 本机处理',
                        'Within-meeting voice matching · Continuous MOSS speaker identity · On-device processing',
                      )
                    : m.id === 'zipformer-en'
                      ? t(
                          '仅英语 · 轻量流式识别 · 使用所选 AI 服务或本地模型翻译',
                          'English only · Lightweight streaming · Translate with your AI service or local model',
                        )
                      : t(
                          '中英文流式识别 · 使用所选 AI 服务或本地模型翻译',
                          'Streaming Chinese and English · Translate with your AI service or local model',
                        )}
              </p>
              {m.id === 'moss' && (
                <p className="mt-2 text-xs leading-6 text-muted-foreground">
                  {m.ready
                    ? t('模型已下载', 'Model downloaded')
                    : t('模型未下载', 'Model not downloaded')}{' '}
                  ·{' '}
                  {data.mossRuntime.ready
                    ? t('运行环境已安装', 'Runtime installed')
                    : t(
                        `首次使用另需下载 ${Math.ceil(data.mossRuntime.downloadBytes / 1e6)} MB 运行环境，安装后约占 1 GB`,
                        `First use also requires a ${Math.ceil(data.mossRuntime.downloadBytes / 1e6)} MB runtime download (about 1 GB installed)`,
                      )}
                </p>
              )}
            </div>
            <Button
              variant="outline"
              size="sm"
              disabled={!!pending || !!data.active || (m.ready && data.busy)}
              onClick={() =>
                void act(m.ready ? 'meeting_model_remove' : 'meeting_model_download', m.id)
              }
            >
              {m.ready
                ? t('移除模型', 'Remove model')
                : m.id === 'moss' && !data.mossRuntime.ready
                  ? t('下载并安装', 'Download and install')
                  : t('下载模型', 'Download model')}
            </Button>
            {m.id === 'moss' && (
              <div className="flex w-full flex-wrap items-center justify-between gap-3 rounded-lg bg-muted/40 px-4 py-3">
                <p className="text-xs text-muted-foreground">
                  {t(
                    '运行环境仅供 MOSS 使用，移除不影响模型权重、录音和纪要。',
                    'The runtime is used only by MOSS. Removing it keeps model weights, recordings and notes.',
                  )}
                </p>
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={!!pending || !!data.active || data.busy}
                  onClick={() =>
                    void act(
                      data.mossRuntime.ready
                        ? 'meeting_moss_runtime_remove'
                        : 'meeting_moss_runtime_download',
                      'moss-runtime',
                    )
                  }
                >
                  {data.mossRuntime.ready
                    ? t('移除运行环境', 'Remove runtime')
                    : t('安装运行环境', 'Install runtime')}
                </Button>
              </div>
            )}
          </div>
        ))}
      </div>
      {data?.active === 'download' && (
        <div className="mt-3 flex items-center gap-4 text-xs">
          <span>{meetingDownloadLabel(data, t)}</span>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => void act('meeting_text_cancel', 'cancel')}
          >
            {t('取消', 'Cancel')}
          </Button>
        </div>
      )}
      {(error || pollError || data?.error) && (
        <p role="alert" className="mt-3 text-sm text-destructive">
          {meetingMessage(error || pollError || data?.error, t)}
        </p>
      )}
    </section>
  )
}
