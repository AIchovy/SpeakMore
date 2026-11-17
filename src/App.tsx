import { MeetingActivity } from '@/features/meeting-activity'
import { lazy, Suspense, useCallback, useEffect, useRef, useState, type ComponentType } from 'react'
import { isTauri } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { Link, Navigate, Route, Routes, useLocation, useNavigate } from 'react-router-dom'
import { MotionConfig, motion } from 'motion/react'
import { Command } from 'cmdk'
import { Toaster, toast } from 'sonner'
import { useApp, useText } from '@/lib/app-state'
import { asAppError, errorMessage } from '@/lib/types'
import { Overview } from '@/features/overview'
import { Dictionary } from '@/features/dictionary'
import { History } from '@/features/history'
import { PiNotepad, PiSubtitles } from 'react-icons/pi'
import { Settings } from '@/features/settings'
import { PermissionRestartPrompt } from '@/features/permission-restart'
import { Modes } from '@/features/modes'
import { StorageRecovery, StorageBackupNotice, useStorageStatus } from '@/features/storage-recovery'
import { Button } from '@/components/ui/button'
import { SpeakMoreMark } from '@/components/brand'
import { TooltipProvider } from '@/components/ui/tooltip'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  SidebarProvider,
  Sidebar,
  SidebarHeader,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarMenu,
  SidebarMenuItem,
  SidebarMenuButton,
  SidebarTrigger,
  useSidebar,
} from '@/components/ui/sidebar'
import {
  HomeIcon,
  HistoryIcon,
  DictionaryIcon,
  ModesIcon,
  ModelIcon,
  SettingsIcon,
  SearchIcon,
  HelpIcon,
  SuccessIcon,
  WarningIcon,
  SpinnerIcon,
  ReloadIcon,
  CommandIcon,
} from '@/components/icons'

// Meetings and captions are large; load them on first use instead of at startup.
const loadMeetings = () => import('@/features/meetings')
const loadCaptions = () => import('@/features/captions')
function RouteUnavailable() {
  const t = useText()
  return (
    <div role="alert" className="space-y-4 py-10 text-sm">
      <p>{t('页面未能载入，请重新载入应用。', 'This page could not load. Reload SpeakMore.')}</p>
      <Button variant="outline" onClick={() => window.location.reload()}>
        <ReloadIcon />
        {t('重新载入', 'Reload')}
      </Button>
    </div>
  )
}
const lazyRoute = (load: () => Promise<ComponentType>) =>
  lazy<ComponentType>(() =>
    load().then(
      (component) => ({ default: component }),
      () => ({ default: RouteUnavailable }),
    ),
  )
const Meetings = lazyRoute(() => loadMeetings().then((module) => module.Meetings))
const Captions = lazyRoute(() => loadCaptions().then((module) => module.Captions))

function Navigation() {
  const { data } = useApp(),
    { pathname } = useLocation(),
    t = useText(),
    { setOpenMobile, isMobile } = useSidebar()
  return (
    <Sidebar collapsible="icon" className="border-r">
      <SidebarHeader className="px-5 pt-8 pb-9 group-data-[collapsible=icon]:px-3">
        <Link
          to="/overview"
          aria-label="SpeakMore"
          className="flex items-center gap-2.5 text-foreground"
        >
          <span className="grid size-8 shrink-0 place-items-center text-black dark:text-white">
            <SpeakMoreMark className="size-8" />
          </span>
          <span className="text-[19px] font-semibold tracking-[-.04em] group-data-[collapsible=icon]:hidden">
            SpeakMore
          </span>
        </Link>
      </SidebarHeader>
      <SidebarContent>
        <SidebarGroup className="px-3">
          <SidebarMenu className="gap-2">
            {[
              ['/overview', HomeIcon, t('概览', 'Overview')],
              ['/meetings', PiNotepad, t('会议记录', 'Meetings')],
              ['/captions', PiSubtitles, t('实时字幕', 'Live captions')],
              ['/history', HistoryIcon, t('历史记录', 'History')],
              ['/dictionary', DictionaryIcon, t('词典', 'Dictionary')],
              ['/modes', ModesIcon, t('听写整理', 'Dictation refinement')],
              ['/settings/models', ModelIcon, t('语音模型', 'Voice models')],
            ].map(([path, Icon, label]) => {
              const Glyph = Icon as typeof HomeIcon
              return (
                <SidebarMenuItem key={path as string}>
                  <SidebarMenuButton
                    asChild
                    isActive={pathname === path}
                    tooltip={label as string}
                    className="h-10 gap-3 px-3 [&>svg]:size-[18px]"
                  >
                    <Link
                      to={path as string}
                      onClick={() => {
                        if (isMobile) setOpenMobile(false)
                      }}
                    >
                      <Glyph />
                      <span>{label as string}</span>
                    </Link>
                  </SidebarMenuButton>
                </SidebarMenuItem>
              )
            })}
          </SidebarMenu>
        </SidebarGroup>
      </SidebarContent>
      <SidebarFooter className="gap-5 px-3 pb-5">
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton
              asChild
              isActive={pathname.startsWith('/settings') && pathname !== '/settings/models'}
              className="h-10 gap-3 px-3 [&>svg]:size-[18px]"
              tooltip={t('设置', 'Settings')}
            >
              <Link to="/settings/general" onClick={() => setOpenMobile(false)}>
                <SettingsIcon />
                <span>{t('设置', 'Settings')}</span>
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
        <div className="mx-3 group-data-[collapsible=icon]:hidden">
          <div className="flex items-center justify-between text-xs text-muted-foreground">
            <span>{data?.desktop ? 'SpeakMore' : t('开发预览', 'Development preview')}</span>
            <span>0.1.0</span>
          </div>
          {!data?.desktop && (
            <p className="mt-2 text-xs leading-5 text-muted-foreground">
              {t('浏览器中的修改仅保留至刷新', 'Browser changes last until reload')}
            </p>
          )}
        </div>
      </SidebarFooter>
    </Sidebar>
  )
}

function Application() {
  const storage = useStorageStatus()
  const { data, error, refresh, save } = useApp(),
    t = useText(),
    location = useLocation(),
    navigate = useNavigate()
  const [search, setSearch] = useState(false),
    [help, setHelp] = useState(false)
  // navigate changes with every location; keep native listeners subscribed once
  // so events are not lost while they would be resubscribing.
  const latest = useRef({ navigate, t })
  useEffect(() => {
    latest.current = { navigate, t }
  })
  // Optimistic sidebar state. Clicks while a save is running only update the
  // wanted value, which is saved next with the refreshed settings revision.
  const [sidebarOpen, setSidebarOpen] = useState<boolean | null>(null)
  const sidebarWanted = useRef<boolean | null>(null),
    sidebarSaving = useRef(false)
  const changeSidebar = useCallback(
    (open: boolean) => {
      setSidebarOpen(open)
      sidebarWanted.current = open
      if (sidebarSaving.current) return
      sidebarSaving.current = true
      void (async () => {
        let retried = false
        try {
          while (sidebarWanted.current !== null) {
            const next = sidebarWanted.current
            sidebarWanted.current = null
            try {
              await save({ sidebarCollapsed: !next })
            } catch (e) {
              // A layout preference: retry a conflict once with the reloaded revision.
              if (retried || asAppError(e).code !== 'RevisionConflict') throw e
              retried = true
              sidebarWanted.current ??= next
            }
          }
        } catch (e) {
          sidebarWanted.current = null
          toast.error(errorMessage(e, latest.current.t))
        } finally {
          sidebarSaving.current = false
          setSidebarOpen(null)
        }
      })()
    },
    [save],
  )
  useEffect(() => {
    // Warm the route chunks after startup so opening them stays instant.
    const timer = setTimeout(() => {
      void loadMeetings().catch(() => {})
      void loadCaptions().catch(() => {})
    }, 3000)
    return () => clearTimeout(timer)
  }, [])
  useEffect(() => {
    if (!isTauri()) return
    let disposed = false
    let off: UnlistenFn | undefined
    void listen<string>('show_meeting_recording', ({ payload }) => {
      if (/^[0-9a-f-]{36}$/i.test(payload)) latest.current.navigate(`/meetings?meeting=${payload}`)
    })
      .then((fn) => {
        if (disposed) fn()
        else off = fn
      })
      .catch(() => {})
    return () => {
      disposed = true
      off?.()
    }
  }, [])
  useEffect(() => {
    if (!isTauri()) return
    let disposed = false
    let off: UnlistenFn | undefined
    void listen('show_recording_results', () => latest.current.navigate('/overview?results=all'))
      .then((unlisten) => {
        if (disposed) unlisten()
        else off = unlisten
      })
      .catch(() => {
        if (!disposed)
          toast.error(
            latest.current.t(
              '无法连接结果提示，请重新打开应用。',
              'Result notifications are unavailable. Reopen SpeakMore.',
            ),
          )
      })
    return () => {
      disposed = true
      off?.()
    }
  }, [])
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault()
        setSearch((s) => !s)
      }
    }
    window.addEventListener('keydown', key)
    return () => window.removeEventListener('keydown', key)
  }, [])
  if (!data && storage?.error) return <StorageRecovery status={storage} />
  if (!data)
    return (
      <div className="grid min-h-screen place-items-center px-6">
        <div className="max-w-md text-center">
          <SpeakMoreMark className="mx-auto mb-6 size-10 text-black dark:text-white" />
          <h1 className="text-xl font-semibold">SpeakMore</h1>
          {error ? (
            <>
              <p role="alert" className="mt-4 text-sm leading-7 text-destructive">
                {typeof error === 'string' ? error : errorMessage(error, t)}
              </p>
              <Button
                className="mt-5"
                onClick={() => void refresh().catch((e) => toast.error(errorMessage(e, t)))}
              >
                <ReloadIcon />
                {t('重试', 'Retry')}
              </Button>
            </>
          ) : (
            <p className="mt-4 text-muted-foreground">
              {t('正在读取本地设置…', 'Reading local settings…')}
            </p>
          )}
        </div>
      </div>
    )
  const links = [
    ['/overview', t('概览', 'Overview')],
    ['/meetings', t('会议记录', 'Meetings')],
    ['/captions', t('实时字幕', 'Live captions')],
    ['/history', t('历史记录', 'History')],
    ['/dictionary', t('词典', 'Dictionary')],
    ['/modes', t('听写整理', 'Dictation refinement')],
    ['/settings/models', t('本地模型', 'Local models')],
    ['/settings/general', t('通用设置', 'General settings')],
    ['/settings/apps', t('应用规则', 'App rules')],
    ['/settings/privacy', t('隐私与数据', 'Privacy & data')],
  ]
  const active = links.find(([path]) => location.pathname === path)?.[1] ?? t('设置', 'Settings')
  return (
    <MotionConfig
      reducedMotion={data.settings.motionPolicy === 'reduced' ? 'always' : 'user'}
      transition={{ duration: 0.14, ease: [0.22, 1, 0.36, 1] }}
    >
      <SidebarProvider
        open={sidebarOpen ?? !data.settings.sidebarCollapsed}
        onOpenChange={changeSidebar}
      >
        <Navigation />
        <div className="min-w-0 flex-1">
          {storage && (
            <StorageBackupNotice status={storage} english={data.settings.uiLocale === 'en'} />
          )}
          <header className="flex h-16 items-center justify-between gap-5 px-6 md:px-8">
            <div className="flex items-center gap-3">
              <SidebarTrigger aria-label={t('展开或收起导航', 'Toggle navigation')} />
              <span className="text-sm text-muted-foreground">{active}</span>
            </div>
            <div className="flex items-center gap-2">
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setSearch(true)}
                className="gap-2 text-muted-foreground"
              >
                <SearchIcon />
                <span className="hidden sm:inline">{t('搜索命令', 'Search commands')}</span>
                <kbd className="ml-3 hidden items-center gap-1 rounded border px-1.5 text-[10px] sm:inline-flex">
                  {navigator.platform.startsWith('Mac') ? (
                    <CommandIcon className="size-3" />
                  ) : (
                    'Ctrl'
                  )}{' '}
                  K
                </kbd>
              </Button>
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={t('帮助', 'Help')}
                onClick={() => setHelp(true)}
              >
                <HelpIcon />
              </Button>
            </div>
          </header>
          <main className="mx-auto max-w-[1056px] px-6 pt-8 pb-12 md:px-10 lg:px-12">
            {import.meta.env.VITE_PUBLIC_AUDIO_CHECK === '1' && (
              <aside
                className="mb-8 rounded-lg border border-amber-500/50 bg-amber-500/10 p-4 text-sm"
                aria-label="公开音频测试构建"
              >
                <strong>公开音频测试构建 · 未访问麦克风</strong>
                <p className="mt-1">
                  开始后点击完成，将处理固定的 300
                  秒公开英语样本。采集、设备和麦克风权限状态为测试替身；计时器不代表样本时长。本构建仅验证界面、处理链路和资源占用。
                </p>
              </aside>
            )}
            {/* Outside the keyed container so navigation does not remount it. */}
            <MeetingActivity />
            {/* Keep content readable even while a background WebView defers animation frames. */}
            <motion.div key={location.pathname} initial={{ y: 4 }} animate={{ y: 0 }}>
              <Suspense
                fallback={
                  <p role="status" className="text-sm text-muted-foreground">
                    {t('正在载入…', 'Loading…')}
                  </p>
                }
              >
                <Routes>
                  <Route path="/overview" element={<Overview />} />
                  <Route path="/dictionary" element={<Dictionary />} />
                  <Route path="/history" element={<History />} />
                  <Route path="/meetings" element={<Meetings />} />
                  <Route path="/captions" element={<Captions />} />
                  <Route path="/models" element={<Navigate to="/settings/models" replace />} />
                  <Route path="/modes" element={<Modes />} />
                  <Route path="/settings/:section" element={<Settings />} />
                  <Route path="*" element={<Navigate to="/overview" replace />} />
                </Routes>
              </Suspense>
            </motion.div>
          </main>
        </div>
      </SidebarProvider>
      <Dialog open={search} onOpenChange={setSearch}>
        <DialogContent className="gap-0 overflow-hidden p-0" closeLabel={t('关闭', 'Close')}>
          <DialogHeader className="sr-only">
            <DialogTitle>{t('搜索命令', 'Search commands')}</DialogTitle>
            <DialogDescription>
              {t('跳转到页面，或搜索设置。', 'Find a page or setting.')}
            </DialogDescription>
          </DialogHeader>
          <Command className="bg-popover" label={t('命令菜单', 'Command menu')}>
            <div className="flex items-center gap-3 border-b px-5">
              <SearchIcon className="size-5 text-muted-foreground" />
              <Command.Input
                autoFocus
                placeholder={t('想去哪里？', 'Where would you like to go?')}
                className="h-14 w-full bg-transparent pr-8 outline-none"
              />
            </div>
            <Command.List className="max-h-[340px] overflow-y-auto p-2">
              <Command.Empty className="py-8 text-center text-sm text-muted-foreground">
                {t('没有匹配的页面', 'No matching pages')}
              </Command.Empty>
              {links.map(([path, label]) => (
                <Command.Item
                  key={path}
                  value={label}
                  onSelect={() => {
                    navigate(path)
                    setSearch(false)
                  }}
                  className="cursor-pointer rounded-md px-4 py-3 text-sm data-[selected=true]:bg-accent data-[selected=true]:text-accent-foreground"
                >
                  {label}
                </Command.Item>
              ))}
            </Command.List>
          </Command>
        </DialogContent>
      </Dialog>
      <Dialog open={help} onOpenChange={setHelp}>
        <DialogContent closeLabel={t('关闭', 'Close')}>
          <DialogHeader>
            <DialogTitle>{t('关于此开发版本', 'About this development build')}</DialogTitle>
            <DialogDescription>
              {t(
                'SpeakMore 正在根据产品设计实现。',
                'SpeakMore is being implemented from its product specification.',
              )}
            </DialogDescription>
          </DialogHeader>
          <p className="text-sm leading-7 text-muted-foreground">
            {t(
              '桌面版已接入本地模型下载与选择、听写控制和结果复制。快捷键、自动输入与 AI 服务仍待完整验收，整体尚未通过发布验收。浏览器环境仅预览界面，不录音或修改桌面数据。',
              'The desktop build connects model downloads and selection, dictation controls, and result copying. Release validation for shortcuts, automatic input, and AI services is incomplete. Browser preview does not record audio or change desktop data.',
            )}
          </p>
        </DialogContent>
      </Dialog>
      <Toaster
        richColors
        position="bottom-right"
        icons={{
          success: <SuccessIcon />,
          error: <WarningIcon />,
          warning: <WarningIcon />,
          info: <HelpIcon />,
          loading: <SpinnerIcon />,
        }}
      />
    </MotionConfig>
  )
}
export default function App() {
  return (
    <TooltipProvider>
      <Application />
      <PermissionRestartPrompt />
    </TooltipProvider>
  )
}
