import { asAppError, type UiMessage } from '@/lib/errors'
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from 'react'
import { invoke, isTauri } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { createPreview } from './preview'
import { LiveSessionStore, mergeSnapshot, type LiveSession } from './snapshot-store'
import { type Snapshot, type SettingsPatch } from './types'

const preview = createPreview()
export async function call<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  return isTauri() ? invoke<T>(command, args) : (preview(command, args) as Promise<T>)
}
interface Context {
  // `data.session.elapsedMs` and `data.audioLevel` only change together with
  // other state; read live values with useLiveSession().
  data: Snapshot | null
  error: UiMessage | null
  pending: boolean
  permissionRestart: 'granted' | 'unconfirmed' | null
  requestPermissionRestart: () => void
  dismissPermissionRestart: () => void
  refresh: () => Promise<void>
  save: (patch: SettingsPatch) => Promise<void>
  mutate: <T>(command: string, args: Record<string, unknown>) => Promise<T>
}
const AppContext = createContext<Context | null>(null)
const LiveContext = createContext<LiveSessionStore | null>(null)
const LocaleContext = createContext<string | null>(null)
const isRevisionConflict = (e: unknown) =>
  typeof e === 'object' && e !== null && 'code' in e && e.code === 'RevisionConflict'
export function AppState({ children }: { children: ReactNode }) {
  const [permissionRestart, setPermissionRestart] = useState<'granted' | 'unconfirmed' | null>(null)
  const previousAccess = useRef<boolean | undefined>(undefined)
  const visitingAccessibility = useRef(false)
  const [data, setData] = useState<Snapshot | null>(null),
    [error, setError] = useState<UiMessage | null>(null),
    [pending, setPending] = useState(false)
  const [levels] = useState(() => new LiveSessionStore())
  const live = useRef(true)
  const latest = useRef<Snapshot | null>(null),
    seenRevision = useRef(-1)
  const running = useRef<Promise<void> | null>(null),
    trailing = useRef<Promise<void> | null>(null),
    mutations = useRef(0)
  const refresh = useCallback(
    function refresh(): Promise<void> {
      // Coalesce bursts of events: one request in flight plus one trailing
      // request, shared by every caller that arrives meanwhile.
      if (running.current)
        return (trailing.current ??= running.current
          .catch(() => {})
          .then(() => {
            trailing.current = null
            return refresh()
          }))
      const request = call<Snapshot>('get_snapshot')
        .then((next) => {
          if (!live.current) return
          setError(null)
          if (next.revision <= seenRevision.current) return
          seenRevision.current = next.revision
          levels.set(next.session.elapsedMs, next.audioLevel)
          const merged = mergeSnapshot(latest.current, next)
          if (merged === latest.current) return
          latest.current = merged
          setData(merged)
        })
        .finally(() => {
          running.current = null
        })
      running.current = request
      return request
    },
    [levels],
  )
  useEffect(() => {
    if (!data?.desktop) return
    const allowed = data.permissions.accessibility
    if (previousAccess.current === false && allowed) {
      visitingAccessibility.current = false
      setPermissionRestart('granted')
    }
    previousAccess.current = allowed
  }, [data?.desktop, data?.permissions.accessibility])
  useEffect(() => {
    const returned = () => {
      if (!visitingAccessibility.current || document.visibilityState === 'hidden') return
      visitingAccessibility.current = false
      setPermissionRestart((current) => current ?? 'unconfirmed')
      void refresh().catch((error) => setError(asAppError(error)))
    }
    window.addEventListener('focus', returned)
    document.addEventListener('visibilitychange', returned)
    return () => {
      window.removeEventListener('focus', returned)
      document.removeEventListener('visibilitychange', returned)
    }
  }, [refresh])
  useEffect(() => {
    live.current = true
    const subscriptions: UnlistenFn[] = []
    let disposed = false
    void (async () => {
      try {
        if (isTauri())
          for (const name of [
            'settings_changed',
            'session_changed',
            'readiness_changed',
            'result_ready',
          ]) {
            const off = await listen(name, () => {
              void refresh().catch((e) => setError(asAppError(e)))
            })
            if (disposed) off()
            else subscriptions.push(off)
          }
        if (!disposed) await refresh()
      } catch (e) {
        if (!disposed) setError(asAppError(e))
      }
    })()
    return () => {
      disposed = true
      live.current = false
      subscriptions.forEach((off) => off())
    }
  }, [refresh])
  useEffect(() => {
    if (!data) return
    const media = matchMedia('(prefers-color-scheme: dark)')
    const apply = () =>
      document.documentElement.classList.toggle(
        'dark',
        data.settings.theme === 'dark' || (data.settings.theme === 'system' && media.matches),
      )
    apply()
    media.addEventListener('change', apply)
    document.documentElement.lang = data.settings.uiLocale
    document.documentElement.dataset.motion = data.settings.motionPolicy
    return () => media.removeEventListener('change', apply)
  }, [data?.settings.theme, data?.settings.uiLocale, data?.settings.motionPolicy])
  const mutate = useCallback(
    async <T,>(command: string, args: Record<string, unknown>): Promise<T> => {
      mutations.current += 1
      setPending(true)
      if (command === 'open_accessibility_settings') visitingAccessibility.current = true
      try {
        let result: T
        try {
          result = await call<T>(command, args)
        } catch (e) {
          if (command === 'open_accessibility_settings') visitingAccessibility.current = false
          if (isRevisionConflict(e)) await refresh().catch(() => {})
          throw e
        }
        // The command succeeded. A failed reload must not report it as failed,
        // or retrying would repeat it; the next native event reloads again.
        await refresh().catch((e) => {
          if (live.current) setError(asAppError(e))
        })
        return result
      } finally {
        mutations.current -= 1
        if (live.current && mutations.current === 0) setPending(false)
      }
    },
    [refresh],
  )
  const save = useCallback(
    async (patch: SettingsPatch) => {
      // Read the newest snapshot so queued saves never reuse a stale revision.
      const current = latest.current
      if (!current) return
      await mutate('update_settings', { expectedRevision: current.settings.revision, patch })
    },
    [mutate],
  )
  const requestPermissionRestart = useCallback(
    () =>
      setPermissionRestart(latest.current?.permissions.accessibility ? 'granted' : 'unconfirmed'),
    [],
  )
  const dismissPermissionRestart = useCallback(() => {
    visitingAccessibility.current = false
    setPermissionRestart(null)
  }, [])
  const value = useMemo(
    () => ({
      data,
      error,
      pending,
      refresh,
      save,
      mutate,
      permissionRestart,
      requestPermissionRestart,
      dismissPermissionRestart,
    }),
    [
      data,
      error,
      pending,
      refresh,
      save,
      mutate,
      permissionRestart,
      requestPermissionRestart,
      dismissPermissionRestart,
    ],
  )
  return (
    <AppContext.Provider value={value}>
      <LiveContext.Provider value={levels}>
        <LocaleContext.Provider value={data?.settings.uiLocale ?? null}>
          {children}
        </LocaleContext.Provider>
      </LiveContext.Provider>
    </AppContext.Provider>
  )
}
export function useApp() {
  const context = useContext(AppContext)
  if (!context) throw new Error('AppState is required')
  return context
}
// Timer and input level of the current dictation, updated about 5 times per second.
export function useLiveSession(): LiveSession {
  const store = useContext(LiveContext)
  if (!store) throw new Error('AppState is required')
  return useSyncExternalStore(store.subscribe, store.snapshot, store.snapshot)
}
// Reads only the interface language, so text-only consumers skip snapshot updates.
export function useText() {
  const locale =
    useContext(LocaleContext) ??
    (typeof navigator !== 'undefined' && !navigator.language.toLowerCase().startsWith('zh')
      ? 'en'
      : 'zh-CN')
  return useCallback((zh: string, en: string) => (locale === 'en' ? en : zh), [locale])
}
