import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { toast } from 'sonner'
import { call, useApp, useText } from '@/lib/app-state'
import { errorMessage, type AppChoice } from '@/lib/types'
import { PageTitle, EmptyState } from '@/components/common'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from '@/components/ui/dialog'
import {
  PiBrowser,
  PiMonitor,
  PiPencilSimple,
  PiPlus,
  PiTrash,
  PiArrowSquareOut,
} from 'react-icons/pi'
import { providerErrorMessage } from './provider-errors'

export interface WritingRule {
  id: string
  kind: 'app' | 'website'
  target: string
  displayName: string
  prompt: string
  enabled: boolean
}
interface Rules {
  revision: number
  items: WritingRule[]
}
export function Modes() {
  const { data, save, pending, mutate } = useApp(),
    t = useText()
  const [rules, setRules] = useState<Rules | null>(null)
  const [prompt, setPrompt] = useState('')
  const [draft, setDraft] = useState<WritingRule | null>(null)
  const [removing, setRemoving] = useState<WritingRule | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<unknown>(null)
  const defaultPrompt =
    (data?.settings.polishMode === 'custom' ? data.settings.customPrompt : '') ||
    (data?.settings.polishMode === 'email'
      ? t(
          '自然分段，整理成清晰的邮件正文，不添加主题或签名。',
          'Use clear email body paragraphs without adding a subject or signature.',
        )
      : data?.settings.polishMode === 'code_comment'
        ? t(
            '整理技术说明，保留术语，不生成代码。',
            'Refine technical explanations, preserve terminology and do not generate code.',
          )
        : '')
  useEffect(() => {
    setPrompt(defaultPrompt)
  }, [defaultPrompt])
  useEffect(() => {
    let live = true
    void call<Rules>('list_writing_rules')
      .then((v) => {
        if (live) setRules(v)
      })
      .catch((e) => {
        if (live) setError(e)
      })
    return () => {
      live = false
    }
  }, [data?.settings.revision])
  if (!data) return null
  const context = data.writingContext
  const act = async (task: () => Promise<void>) => {
    if (busy) return
    setBusy(true)
    setError(null)
    try {
      await task()
    } catch (e) {
      setError(e)
    } finally {
      setBusy(false)
    }
  }
  const add = (kind: 'app' | 'website') =>
    setDraft({
      id: crypto.randomUUID(),
      kind,
      target: '',
      displayName: '',
      prompt: '',
      enabled: true,
    })
  const choose = () =>
    void act(async () => {
      const app = await call<AppChoice | null>('choose_app_for_rule')
      if (app)
        setDraft((d) => (d ? { ...d, target: app.appIdentity, displayName: app.displayName } : d))
    })
  return (
    <>
      <PageTitle
        title={t('听写整理', 'Dictation refinement')}
        description={t(
          '根据你正在输入的应用和网页，自动整理表达。',
          'Adapt your dictated words to the app and page you are writing in.',
        )}
      />
      <section className="flex items-start justify-between gap-6 border-b pb-6">
        <div>
          <Label htmlFor="polish-enabled" className="text-base">
            {t('AI 自动整理', 'Automatic AI refinement')}
          </Label>
          <p className="mt-2 max-w-xl text-sm leading-6 text-muted-foreground">
            {t(
              '听写开始时识别输入场景，使用当前 AI 服务整理文字。',
              'Detects your writing context when dictation starts and refines text with your current AI service.',
            )}
          </p>
          <Link
            className="mt-3 inline-flex items-center gap-1.5 text-sm underline underline-offset-4"
            to="/settings/ai"
          >
            {t('配置 AI 服务', 'Configure AI service')}
            <PiArrowSquareOut />
          </Link>
        </div>
        <Switch
          id="polish-enabled"
          aria-label={t('AI 自动整理', 'Automatic AI refinement')}
          checked={data.settings.polishEnabled}
          disabled={
            pending ||
            (!data.settings.polishEnabled &&
              (!data.settings.activeProviderId || data.session.phase !== 'idle'))
          }
          onCheckedChange={(enabled) =>
            void (
              enabled
                ? mutate('enable_polishing', { expectedRevision: data.settings.revision })
                : save({ polishEnabled: false })
            ).catch((e) => toast.error(providerErrorMessage(e, t)))
          }
        />
      </section>
      <section className="py-7">
        <div className="mb-3 flex items-center justify-between gap-4">
          <Label htmlFor="default-writing-prompt" className="text-base">
            {t('默认整理要求', 'Default editing preference')}
          </Label>
          <Button
            size="sm"
            variant="outline"
            disabled={
              pending ||
              busy ||
              prompt === defaultPrompt ||
              new TextEncoder().encode(prompt).length > 4096
            }
            onClick={() =>
              void save({ customPrompt: prompt, polishMode: prompt.trim() ? 'custom' : 'standard' })
                .then(() => toast.success(t('默认要求已保存', 'Default preference saved')))
                .catch((e) => setError(e))
            }
          >
            {t('保存', 'Save')}
          </Button>
        </div>
        <p className="mb-3 text-sm leading-6 text-muted-foreground">
          {t(
            '未匹配规则时，结合应用和窗口标题自动调整语气与分段。可填写你的通用偏好。',
            'When no rule matches, use app and window context to adapt tone and paragraphs. Add your general preferences here.',
          )}
        </p>
        <Textarea
          id="default-writing-prompt"
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
          className="min-h-28 resize-y"
          placeholder={t(
            '例如：保留我的语气，表达简洁；聊天自然，邮件清晰分段。',
            'For example: Keep my voice and be concise. Use a natural tone in chat and clear paragraphs in email.',
          )}
        />
        <p className="mt-2 text-right text-xs text-muted-foreground">
          {new TextEncoder().encode(prompt).length} / 4096 bytes
        </p>
      </section>
      <section className="border-t pt-7">
        <div className="mb-5 flex flex-wrap items-start justify-between gap-4">
          <div>
            <h2 className="text-base font-medium">
              {t('应用与网站规则', 'App and website rules')}
            </h2>
            <p className="mt-2 text-sm text-muted-foreground">
              {t(
                '网站规则优先于应用规则；同一网站中，地址更具体的规则优先。',
                'Website rules override app rules. More specific website paths take priority.',
              )}
            </p>
          </div>
          <div className="flex gap-2">
            <Button
              variant="outline"
              size="sm"
              disabled={busy || !rules || rules.items.length >= 100}
              onClick={() => add('app')}
            >
              <PiPlus />
              {t('添加应用', 'Add app')}
            </Button>
            <Button
              variant="outline"
              size="sm"
              disabled={busy || !rules || rules.items.length >= 100}
              onClick={() => add('website')}
            >
              <PiPlus />
              {t('添加网站', 'Add website')}
            </Button>
          </div>
        </div>
        {rules?.items.length ? (
          <div className="divide-y">
            {rules.items.map((rule) => (
              <div key={rule.id} className="flex items-start gap-4 py-5">
                {rule.kind === 'app' ? (
                  <PiMonitor className="mt-1 size-5 shrink-0 text-muted-foreground" />
                ) : (
                  <PiBrowser className="mt-1 size-5 shrink-0 text-muted-foreground" />
                )}
                <div className="min-w-0 flex-1">
                  <h3 className="text-sm font-medium">{rule.displayName || rule.target}</h3>
                  <p className="mt-1 break-all text-xs text-muted-foreground">{rule.target}</p>
                  <p className="mt-2 line-clamp-2 whitespace-pre-wrap text-sm leading-6 text-muted-foreground">
                    {rule.prompt}
                  </p>
                </div>
                <Switch
                  aria-label={t(
                    `启用 ${rule.displayName || rule.target} 的规则`,
                    `Enable rule for ${rule.displayName || rule.target}`,
                  )}
                  checked={rule.enabled}
                  disabled={busy || pending}
                  onCheckedChange={(enabled) =>
                    void act(async () => {
                      const next = await mutate<Rules>('upsert_writing_rule', {
                        expectedRevision: rules.revision,
                        rule: { ...rule, enabled },
                      })
                      setRules(next)
                    })
                  }
                />
                <Button
                  size="icon"
                  variant="ghost"
                  aria-label={t('编辑规则', 'Edit rule')}
                  disabled={busy}
                  onClick={() => {
                    setError(null)
                    setDraft({ ...rule })
                  }}
                >
                  <PiPencilSimple />
                </Button>
                <Button
                  size="icon"
                  variant="ghost"
                  aria-label={t('删除规则', 'Delete rule')}
                  disabled={busy}
                  onClick={() => {
                    setError(null)
                    setRemoving(rule)
                  }}
                >
                  <PiTrash />
                </Button>
              </div>
            ))}
          </div>
        ) : (
          <EmptyState
            icon={PiBrowser}
            title={t('让常用场景有自己的表达', 'Set a preference for each writing context')}
            description={t(
              '为微信设置自然简短的聊天语气，或为邮件网站设置正式、清晰的段落。',
              'Use a brief, natural tone in chat apps or clear, formal paragraphs on an email website.',
            )}
          />
        )}
      </section>
      {context && (
        <section className="mt-7 rounded-lg bg-muted/40 px-4 py-4 text-xs leading-6">
          <h2 className="font-medium">
            {t('最近识别的输入场景', 'Last detected writing context')}
          </h2>
          <p className="mt-1 break-words">
            {context.appName || context.appIdentity || t('未识别应用', 'App unavailable')}
            {context.windowTitle ? ` · ${context.windowTitle}` : ''}
          </p>
          {context.inputTitle && <p className="text-muted-foreground">{context.inputTitle}</p>}
          {context.websiteUrl && (
            <p className="break-all text-muted-foreground">{context.websiteUrl}</p>
          )}
          <p className="mt-1 text-muted-foreground">
            {t('使用规则：', 'Applied preference: ')}
            {data.writingRule || t('默认整理要求', 'Default editing preference')}
          </p>
        </section>
      )}
      <p className="mt-6 max-w-3xl text-xs leading-6 text-muted-foreground">
        {t(
          '仅在听写开始时读取应用、窗口／输入框标题和可获取的网页地址。网页地址读不到时使用应用规则。选择云端 AI 服务时，这些场景信息会和听写文字一起发送。规则修改从下一次听写生效，整理失败时保留原文。',
          'Reads app, window or input titles and available page URLs only when dictation starts. If a page URL is unavailable, app rules apply. With a cloud AI service, this context is sent alongside dictated text. Rule changes apply to the next dictation; the original is kept if refinement fails.',
        )}
      </p>
      {!!error && !draft && !removing && (
        <p role="alert" className="mt-4 text-sm text-destructive">
          {errorMessage(error, t)}
        </p>
      )}
      <Dialog
        open={!!draft}
        onOpenChange={(open) => {
          if (!open && !busy) {
            setDraft(null)
            setError(null)
          }
        }}
      >
        <DialogContent className="sm:max-w-[620px]" closeLabel={t('关闭', 'Close')}>
          <DialogHeader>
            <DialogTitle>
              {draft?.kind === 'app'
                ? t('应用整理规则', 'App editing rule')
                : t('网站整理规则', 'Website editing rule')}
            </DialogTitle>
            <DialogDescription>
              {t(
                '匹配到这个场景时，使用下方要求整理你的听写。',
                'Use the preference below whenever this writing context matches.',
              )}
            </DialogDescription>
          </DialogHeader>
          {draft && (
            <div className="space-y-5 py-2">
              {draft.kind === 'app' ? (
                <div>
                  <Label>{t('应用', 'App')}</Label>
                  <div className="mt-3 flex items-center justify-between gap-3 rounded-lg border px-3 py-3">
                    <div className="min-w-0">
                      <p className="text-sm">
                        {draft.displayName || t('尚未选择应用', 'No app selected')}
                      </p>
                      {draft.target && (
                        <p className="mt-1 break-all text-xs text-muted-foreground">
                          {draft.target}
                        </p>
                      )}
                    </div>
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={busy || !data.desktop}
                      onClick={choose}
                    >
                      {t('选择应用', 'Choose app')}
                    </Button>
                  </div>
                  {!data.desktop && (
                    <p className="mt-2 text-xs text-muted-foreground">
                      {t(
                        '请在桌面应用中选择已安装的应用。',
                        'Choose an installed app in the desktop application.',
                      )}
                    </p>
                  )}
                </div>
              ) : (
                <div className="space-y-3">
                  <Label htmlFor="writing-url">{t('网站地址', 'Website address')}</Label>
                  <Input
                    id="writing-url"
                    value={draft.target}
                    onChange={(e) => setDraft({ ...draft, target: e.target.value })}
                    placeholder="example.com/mail"
                  />
                  <p className="text-xs leading-5 text-muted-foreground">
                    {t(
                      '填写域名匹配整个网站，填写路径只匹配该路径及其子页面。不使用查询参数和锚点。',
                      'A domain matches the entire site. A path matches that path and its subpages. Query parameters and fragments are ignored.',
                    )}
                  </p>
                  <Label htmlFor="writing-name">{t('名称（可选）', 'Name (optional)')}</Label>
                  <Input
                    id="writing-name"
                    maxLength={120}
                    value={draft.displayName}
                    onChange={(e) => setDraft({ ...draft, displayName: e.target.value })}
                    placeholder={t('例如：工作邮箱', 'For example: Work email')}
                  />
                </div>
              )}
              <div className="space-y-3">
                <Label htmlFor="writing-rule-prompt">{t('整理提示词', 'Editing prompt')}</Label>
                <Textarea
                  id="writing-rule-prompt"
                  className="min-h-40"
                  value={draft.prompt}
                  onChange={(e) => setDraft({ ...draft, prompt: e.target.value })}
                  placeholder={t(
                    '例如：用自然、简短的聊天语气，保留专业术语，不添加开场白。',
                    'For example: Use a natural, concise chat tone. Keep technical terms and add no greetings.',
                  )}
                />
                <p className="text-right text-xs text-muted-foreground">
                  {new TextEncoder().encode(draft.prompt).length} / 4096 bytes
                </p>
              </div>
            </div>
          )}
          {!!error && (
            <p role="alert" className="text-sm text-destructive">
              {errorMessage(error, t)}
            </p>
          )}
          <DialogFooter>
            <Button
              variant="outline"
              disabled={busy}
              onClick={() => {
                setDraft(null)
                setError(null)
              }}
            >
              {t('取消', 'Cancel')}
            </Button>
            <Button
              disabled={
                busy ||
                pending ||
                !rules ||
                !draft?.target.trim() ||
                !draft.prompt.trim() ||
                new TextEncoder().encode(draft.prompt).length > 4096
              }
              onClick={() =>
                void act(async () => {
                  if (!draft || !rules) return
                  setRules(
                    await mutate<Rules>('upsert_writing_rule', {
                      expectedRevision: rules.revision,
                      rule: { ...draft, displayName: draft.displayName.trim() || draft.target },
                    }),
                  )
                  setDraft(null)
                  toast.success(t('规则已保存', 'Rule saved'))
                })
              }
            >
              {t('保存规则', 'Save rule')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog
        open={!!removing}
        onOpenChange={(open) => {
          if (!open && !busy) {
            setRemoving(null)
            setError(null)
          }
        }}
      >
        <DialogContent closeLabel={t('关闭', 'Close')}>
          <DialogHeader>
            <DialogTitle>{t('删除这条整理规则？', 'Delete this editing rule?')}</DialogTitle>
            <DialogDescription>{removing?.displayName || removing?.target}</DialogDescription>
          </DialogHeader>
          {!!error && (
            <p role="alert" className="text-sm text-destructive">
              {errorMessage(error, t)}
            </p>
          )}
          <DialogFooter>
            <Button variant="outline" disabled={busy} onClick={() => setRemoving(null)}>
              {t('取消', 'Cancel')}
            </Button>
            <Button
              variant="destructive"
              disabled={busy || pending}
              onClick={() =>
                void act(async () => {
                  if (!removing || !rules) return
                  setRules(
                    await mutate<Rules>('delete_writing_rule', {
                      expectedRevision: rules.revision,
                      id: removing.id,
                    }),
                  )
                  setRemoving(null)
                })
              }
            >
              {t('删除规则', 'Delete rule')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
