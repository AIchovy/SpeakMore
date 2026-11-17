import { asAppError, type UiMessage } from '@/lib/errors'
import { useEffect, useState } from 'react'
import { invoke, isTauri } from '@tauri-apps/api/core'
import { Button } from '@/components/ui/button'
import { ShieldIcon, SuccessIcon, WarningIcon } from '@/components/icons'
import { errorMessage } from '@/lib/types'

type Snapshot = {
  revision: number
  phase: 'idle' | 'countdown' | 'capturing' | 'observing' | 'passed' | 'failed' | 'cancelled'
  accessibility: boolean
  remainingMs: number
  reason: string | null
  operation: string | null
  nativeError: number | null
}
const reasons: Record<string, string> = {
  accessibility_required: '辅助功能权限尚未允许。',
  unsupported_target: '当前应用或编辑器结构不符合候选规则。',
  target_unavailable: '无法完整读取目标的原生属性或通知。',
  target_changed: '观察期间目标、焦点、选区或文字发生变化。',
  probe_timeout: '原生读取超过 300 毫秒预算。',
  sensitive_target: '此输入位置属于敏感控件。',
  read_only_target: '此输入位置不可编辑。',
}

export default function TargetAudit() {
  const [data, setData] = useState<Snapshot | null>(null)
  const [error, setError] = useState<UiMessage | null>(null)
  const [pending, setPending] = useState(false)
  useEffect(() => {
    let disposed = false
    let timer: ReturnType<typeof setTimeout>
    async function refresh() {
      try {
        if (!isTauri()) throw new Error('请使用独立的 SpeakMore Input Check 原生应用。')
        const next = await invoke<Snapshot>('audit_status')
        if (!disposed) setData((old) => (old && old.revision > next.revision ? old : next))
      } catch (failure) {
        if (!disposed) setError(asAppError(failure))
      } finally {
        if (!disposed) timer = setTimeout(() => void refresh(), 200)
      }
    }
    void refresh()
    return () => {
      disposed = true
      clearTimeout(timer)
    }
  }, [])
  async function act(command: string, args: Record<string, unknown> = {}) {
    setPending(true)
    setError(null)
    try {
      await invoke(command, args)
    } catch (failure) {
      setError(asAppError(failure))
    } finally {
      setPending(false)
    }
  }
  const active = data && ['countdown', 'capturing', 'observing'].includes(data.phase)
  const labels = {
    idle: '准备观察',
    countdown: '请切换到 TextEdit 的测试文稿',
    capturing: '正在捕获目标',
    observing: '正在观察目标，请保持光标不动',
    passed: '观察与独立复核完成',
    failed: '目标未通过本次观察',
    cancelled: '本次观察已取消',
  }
  return (
    <main className="mx-auto max-w-3xl px-10 py-12">
      <div className="mb-9 flex items-center gap-3 text-sm text-muted-foreground">
        <ShieldIcon className="size-5" />
        SpeakMore · 独立输入验证
      </div>
      <h1 className="text-3xl font-semibold tracking-tight">原生目标观察</h1>
      <p className="mt-3 leading-7 text-muted-foreground">
        检查 TextEdit
        的纯文本结构、选区与变更通知。此工具只做只读观察，不录音、不粘贴，不打开产品数据库。
      </p>
      <section className="mt-9 rounded-xl border bg-card p-6">
        <div className="flex flex-wrap items-center justify-between gap-4">
          <div>
            <h2 className="font-medium">辅助功能权限</h2>
            <p role="status" className="mt-2 text-sm text-muted-foreground">
              {data?.accessibility ? '已允许独立验证应用' : '尚未允许独立验证应用'}
            </p>
          </div>
          <div className="flex gap-2">
            <Button
              variant="outline"
              disabled={pending || !!active || !!data?.accessibility}
              onClick={() => void act('audit_permission_action', { request: true })}
            >
              请求权限
            </Button>
            <Button
              variant="outline"
              disabled={pending || !!active}
              onClick={() => void act('audit_permission_action', { request: false })}
            >
              打开系统设置
            </Button>
          </div>
        </div>
        <p className="mt-4 text-sm leading-6 text-muted-foreground">
          此工具使用系统辅助功能读取测试目标。请使用不含个人信息的文稿，并主动开始观察。
        </p>
      </section>
      <section className="mt-5 rounded-xl border bg-card p-6">
        <div className="flex items-center gap-3">
          <span aria-hidden>
            {data?.phase === 'passed' ? (
              <SuccessIcon className="size-5" />
            ) : data?.phase === 'failed' ? (
              <WarningIcon className="size-5" />
            ) : (
              <ShieldIcon className="size-5" />
            )}
          </span>
          <h2 role="status" className="font-medium">
            {data ? labels[data.phase] : '正在连接原生验证器'}
          </h2>
        </div>
        {active && (
          <p className="mt-3 text-sm tabular-nums text-muted-foreground">
            剩余 {Math.ceil((data?.remainingMs ?? 0) / 1000)} 秒
          </p>
        )}
        {data?.reason && (
          <p className="mt-3 text-sm">{reasons[data.reason] ?? '本次目标无法确认。'}</p>
        )}
        {data?.operation && (
          <p className="mt-2 text-xs text-muted-foreground">最近检查步骤：{data.operation}</p>
        )}
        {data?.nativeError != null && (
          <p className="mt-2 text-xs text-muted-foreground">原生错误码：{data.nativeError}</p>
        )}
        <p className="mt-4 text-sm leading-6 text-muted-foreground">
          开始后有 8 秒切换到 TextEdit，随后观察 8
          秒。检查变更时，可在观察期间移动光标或切换窗口，应得到目标变化结果。
        </p>
        <div className="mt-5 flex gap-3">
          <Button
            disabled={pending || !data?.accessibility || !!active}
            onClick={() => void act('audit_begin')}
          >
            开始观察
          </Button>
          <Button
            variant="outline"
            disabled={pending || !active}
            onClick={() => void act('audit_cancel')}
          >
            取消观察
          </Button>
        </div>
      </section>
      {error && (
        <p
          role="alert"
          className="mt-5 rounded-lg border border-destructive/30 p-4 text-sm text-destructive"
        >
          {typeof error === 'string' ? error : errorMessage(error, (zh) => zh)}
        </p>
      )}
      <p className="mt-7 text-xs leading-6 text-muted-foreground">
        观察通过不代表自动粘贴已验收。此开发工具不会授予任何原生目标发布兼容性资格。
      </p>
    </main>
  )
}
