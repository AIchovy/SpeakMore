import { useState } from 'react'
import { Link, useParams } from 'react-router-dom'
import { toast } from 'sonner'
import { call, useApp, useText } from '@/lib/app-state'
import { errorMessage, type Settings as Preferences } from '@/lib/types'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { PageTitle, SettingsRow, Choice, Confirm } from '@/components/common'
import { RecordingControls } from './recording-controls'
import { SunIcon, MoonIcon, MonitorIcon, ShieldIcon, MicrophoneIcon } from '@/components/icons'
import { cn } from '@/lib/utils'
import { InputDevices } from './input-devices'
import { Permissions } from './permissions'
import { Models } from './models'
import { AppRules } from './app-rules'
import { Providers } from './providers'
import { CloudPolicy } from './cloud-policy'

export function Settings() {
  const { section = 'general' } = useParams(),
    { data, save, pending, mutate } = useApp(),
    t = useText()
  const [clear, setClear] = useState(false)
  const [clearRecent, setClearRecent] = useState(false)
  if (!data) return null
  const s = data.settings
  const change = (patch: Parameters<typeof save>[0]) =>
    void save(patch).catch((e) => toast.error(errorMessage(e, t)))
  const pages = [
    ['general', t('通用', 'General')],
    ['audio', t('音频与快捷键', 'Audio & shortcuts')],
    ['models', t('本地模型', 'Local models')],
    ['ai', t('AI 服务', 'AI services')],
    ['apps', t('应用规则', 'App rules')],
    ['privacy', t('隐私与数据', 'Privacy & data')],
    ['about', t('关于', 'About')],
  ]
  return (
    <>
      <PageTitle
        title={t('设置', 'Settings')}
        description={t(
          '按你的习惯，让 SpeakMore 融入日常。',
          'Make SpeakMore fit the way you work.',
        )}
      />
      <nav
        aria-label={t('设置分类', 'Settings sections')}
        className="mb-7 flex gap-5 overflow-x-auto border-b"
      >
        {pages.map(([id, label]) => (
          <Link
            key={id}
            to={`/settings/${id}`}
            aria-current={section === id ? 'page' : undefined}
            className={cn(
              'shrink-0 border-b-2 px-1 pb-3 text-sm',
              section === id
                ? 'border-accent-foreground font-medium text-foreground'
                : 'border-transparent text-muted-foreground hover:text-foreground',
            )}
          >
            {label}
          </Link>
        ))}
      </nav>
      <div className="max-w-[840px]">
        {section === 'models' && <Models embedded />}
        {section === 'general' && (
          <>
            <h2 className="mb-1 text-lg font-semibold">
              {t('外观与语言', 'Appearance & language')}
            </h2>
            <SettingsRow
              title={t('外观', 'Appearance')}
              description={t(
                '选择浅色、深色，或跟随系统。',
                'Choose light, dark, or your system preference.',
              )}
            >
              <div className="flex gap-2">
                {(
                  [
                    ['light', SunIcon, t('浅色', 'Light')],
                    ['dark', MoonIcon, t('深色', 'Dark')],
                    ['system', MonitorIcon, t('系统', 'System')],
                  ] as const
                ).map(([v, Icon, label]) => (
                  <Button
                    key={v}
                    aria-pressed={s.theme === v}
                    disabled={pending}
                    variant={s.theme === v ? 'secondary' : 'outline'}
                    className={cn(
                      'min-w-20 shadow-none',
                      s.theme === v &&
                        'border border-accent-foreground/40 bg-accent text-accent-foreground',
                    )}
                    onClick={() => change({ theme: v })}
                  >
                    <Icon />
                    {label}
                  </Button>
                ))}
              </div>
            </SettingsRow>
            <SettingsRow
              title={t('界面语言', 'Display language')}
              description={t(
                '只影响界面文字，不改变语音识别语言。',
                'Changes interface text, not speech recognition.',
              )}
            >
              <Choice
                label={t('界面语言', 'Display language')}
                value={s.uiLocale}
                disabled={pending}
                onChange={(v) => change({ uiLocale: v as Preferences['uiLocale'] })}
                options={[
                  ['zh-CN', '简体中文'],
                  ['en', 'English'],
                ]}
              />
            </SettingsRow>
            <SettingsRow
              title={t('减少动态效果', 'Reduce motion')}
              description={t(
                '简化过渡效果。默认遵循系统的动态效果偏好。',
                'Simplify transitions. System accessibility preferences are always respected.',
              )}
            >
              <Switch
                aria-label={t('减少动态效果', 'Reduce motion')}
                checked={s.motionPolicy === 'reduced'}
                disabled={pending}
                onCheckedChange={(v) => change({ motionPolicy: v ? 'reduced' : 'system' })}
              />
            </SettingsRow>
          </>
        )}
        {section === 'privacy' && (
          <>
            <div className="mb-2 flex items-center gap-2">
              <ShieldIcon className="size-5 text-accent-foreground" />
              <h2 className="text-lg font-semibold">
                {t('你的表达，由你掌握', 'Your words, your choice')}
              </h2>
            </div>
            <CloudPolicy />
            <SettingsRow
              title={t('保存历史记录', 'Save history')}
              description={t(
                '将转写文字保存在本机。关闭后停止保存新记录，已有记录不会自动删除。',
                'Store transcripts on this device. Turning this off stops new saves without deleting existing records.',
              )}
            >
              <Switch
                aria-label={t('保存历史记录', 'Save history')}
                checked={s.historyEnabled}
                disabled={pending}
                onCheckedChange={(v) => change({ historyEnabled: v })}
              />
            </SettingsRow>
            <SettingsRow
              title={t('保留时间', 'Retention')}
              description={t(
                '到期或超出条数上限时自动清理，以更严格者为准。',
                'Records expire by age or count, whichever limit is reached first.',
              )}
            >
              <Choice
                label={t('历史保留时间', 'Retention period')}
                value={String(s.historyDays)}
                disabled={pending}
                options={[
                  ['1', t('1 天', '1 day')],
                  ['7', t('7 天', '7 days')],
                  ['30', t('30 天', '30 days')],
                ]}
                onChange={(v) => change({ historyDays: Number(v) })}
              />
            </SettingsRow>
            <SettingsRow
              title={t('条数上限', 'Record limit')}
              description={t(
                '仅限制持久历史，不影响最近结果。',
                'Applies to saved history, not recent results.',
              )}
            >
              <Choice
                label={t('历史条数上限', 'History limit')}
                value={String(s.historyLimit)}
                disabled={pending}
                options={[100, 500, 1000, 5000].map((n) => [
                  String(n),
                  `${n} ${t('条', 'records')}`,
                ])}
                onChange={(v) => change({ historyLimit: Number(v) })}
              />
            </SettingsRow>
            <SettingsRow
              title={t('文字交付方式', 'Text delivery')}
              description={t(
                '在其他应用的输入框中用快捷键听写，完成后自动粘贴。需要辅助功能权限；中途切换输入位置时保留结果供复制。概览中的听写仍由你主动复制。',
                'Shortcut dictation pastes into the original editable field with Accessibility access. If the target changes, the result remains available to copy. Dictation started from Overview is copied manually.',
              )}
            >
              <Choice
                label={t('文字交付方式', 'Text delivery')}
                value={s.deliveryMode}
                disabled={pending}
                options={[
                  ['paste', t('自动输入到原输入框', 'Paste into original field')],
                  ['manual', t('仅显示结果', 'Show results only')],
                ]}
                onChange={(v) => change({ deliveryMode: v as Preferences['deliveryMode'] })}
                className="w-[220px]"
              />
            </SettingsRow>
            <SettingsRow
              title={t('立即清空结果', 'Clear recent results')}
              description={t(
                '清除内存中的临时文字。已保存的历史记录不受影响。',
                'Clear temporary text from memory. Saved history is unaffected.',
              )}
            >
              <Button
                variant="outline"
                disabled={pending || data.recentResults.length === 0}
                onClick={() => setClearRecent(true)}
              >
                {t('清空最近结果', 'Clear recent results')}
              </Button>
            </SettingsRow>
            <SettingsRow
              title={t('清空历史记录', 'Clear history')}
              description={t(
                '删除保存在本机的转写，系统剪贴板保持不变。',
                'Delete locally saved transcripts. Your clipboard is unaffected.',
              )}
            >
              <Button variant="outline" onClick={() => setClear(true)}>
                {t('清空历史', 'Clear history')}
              </Button>
            </SettingsRow>
            <div className="mt-6 rounded-lg bg-muted px-4 py-3 text-xs leading-6 text-muted-foreground">
              {t(
                '录音音频不保存到磁盘。历史数据库保存在本地，依赖系统账户与磁盘保护；开启云端整理时，仅发送本次转写文字。',
                'Audio recordings aren’t written to disk. Local history relies on your account and disk protection. Cloud refinement, when enabled, sends only the current transcript.',
              )}
            </div>
          </>
        )}
        {section === 'audio' && (
          <>
            <h2 className="text-lg font-semibold">{t('录音与操作', 'Recording controls')}</h2>
            <Permissions />
            <InputDevices />
            <SettingsRow
              title={t('单次录音时长', 'Recording limit')}
              description={t(
                '到时自动结束，并处理完整的有效录音。',
                'Recording stops at the limit and processes all valid audio.',
              )}
            >
              <span className="rounded-lg border bg-card px-4 py-2 font-medium tabular-nums">
                05:00
              </span>
            </SettingsRow>
            <RecordingControls />
          </>
        )}
        {section === 'ai' && <Providers />}
        {section === 'apps' && <AppRules />}
        {section === 'about' && (
          <>
            <h2 className="text-lg font-semibold">SpeakMore</h2>
            <p className="mt-2 text-muted-foreground">
              {t('自然表达，留住思路。', 'Speak naturally. Keep your train of thought.')}
            </p>
            <SettingsRow
              title={t('当前版本', 'Version')}
              description={t(
                '开发版本，尚未通过商业发布验收。',
                'Development build. Commercial release verification is incomplete.',
              )}
            >
              <span className="text-muted-foreground">0.1.0</span>
            </SettingsRow>
            <SettingsRow
              title={t('本期功能', 'Current edition')}
              description={t(
                '听写不限次数，所有本地模型与 AI 文字整理均可使用。云端 AI 使用你配置的服务凭据，费用由服务商收取。',
                'Unlimited dictation, all local models, and AI text refinement. Cloud AI uses your configured credentials; provider charges may apply.',
              )}
            >
              <span className="text-sm text-muted-foreground">
                {t('无需激活', 'No activation')}
              </span>
            </SettingsRow>
          </>
        )}
      </div>
      <Confirm
        open={clearRecent}
        onOpenChange={setClearRecent}
        title={t('清空最近结果？', 'Clear recent results?')}
        description={t(
          '将清除内存中的临时文字，已保存的历史记录和使用次数保持不变。',
          'Removes temporary results from memory. Saved history and usage counts stay as they are.',
        )}
        onConfirm={() => mutate('clear_recent_results', {})}
      />
      <Confirm
        open={clear}
        onOpenChange={setClear}
        title={t('清空全部历史记录？', 'Clear all history?')}
        description={t(
          '删除后无法在 SpeakMore 中恢复。',
          'Deleted records cannot be restored in SpeakMore.',
        )}
        onConfirm={async () => {
          await call('clear_history')
          toast.success(t('历史已清空', 'History cleared'))
        }}
      />
    </>
  )
}
