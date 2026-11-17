import { asAppError, type UiMessage } from '@/lib/errors'
import { useEffect, useRef, useState, type ReactNode } from 'react'
import type { IconType } from 'react-icons'
import { Button } from './ui/button'
import { DialogFooter } from './ui/dialog'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from './ui/select'
import { WarningIcon } from './icons'
import { errorMessage } from '@/lib/types'
import { useText } from '@/lib/app-state'
import { AlertDialog } from 'radix-ui'

export function PageTitle({
  title,
  description,
  action,
}: {
  title: string
  description: string
  action?: ReactNode
}) {
  return (
    <div className="mb-10 flex flex-wrap items-start justify-between gap-4">
      <div>
        <h1 className="text-[30px] leading-tight font-medium tracking-[-.04em] sm:text-[32px]">
          {title}
        </h1>
        <p className="mt-3 max-w-2xl text-sm leading-6 text-muted-foreground">{description}</p>
      </div>
      {action}
    </div>
  )
}
export function EmptyState({
  icon: Icon,
  title,
  description,
  action,
}: {
  icon: IconType
  title: string
  description: string
  action?: ReactNode
}) {
  return (
    <div className="flex flex-col items-center px-6 py-14 text-center">
      <div className="mb-4 grid size-10 place-items-center text-muted-foreground">
        <Icon className="size-6" aria-hidden />
      </div>
      <h3 className="font-medium">{title}</h3>
      <p className="mt-2 max-w-[360px] text-sm leading-6 text-muted-foreground">{description}</p>
      {action && <div className="mt-5">{action}</div>}
    </div>
  )
}
export function InlineError({ message }: { message: UiMessage | null }) {
  const t = useText()
  return message ? (
    <div
      role="alert"
      className="flex gap-2 rounded-lg border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive"
    >
      <WarningIcon className="mt-0.5 size-4 shrink-0" />
      {typeof message === 'string' ? message : errorMessage(message, t)}
    </div>
  ) : null
}
export function Choice({
  value,
  onChange,
  options,
  label,
  disabled = false,
  className = 'w-[180px]',
}: {
  value: string
  onChange: (value: string) => void
  options: [string, string][]
  label: string
  disabled?: boolean
  className?: string
}) {
  const trigger = useRef<HTMLButtonElement>(null)
  const restoreAfterSave = useRef(false)
  useEffect(() => {
    if (disabled || !restoreAfterSave.current) return
    restoreAfterSave.current = false
    // Saving can disable the trigger during Radix's close autofocus. Restore
    // it once enabled, unless the user has already focused another control.
    if (!document.activeElement || document.activeElement === document.body)
      trigger.current?.focus({ preventScroll: true })
  }, [disabled])
  return (
    <Select value={value} onValueChange={onChange} disabled={disabled}>
      <SelectTrigger ref={trigger} aria-label={label} className={className}>
        <SelectValue />
      </SelectTrigger>
      <SelectContent
        onCloseAutoFocus={() => {
          restoreAfterSave.current = trigger.current?.disabled === true
        }}
      >
        {options.map(([v, t]) => (
          <SelectItem key={v} value={v}>
            {t}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  )
}
export function SettingsRow({
  title,
  description,
  children,
  feedback,
}: {
  title: string
  description: string
  children: ReactNode
  feedback?: ReactNode
}) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-5 border-b py-6 last:border-b-0">
      <div className="max-w-[430px]">
        <h3 className="font-medium">{title}</h3>
        <p className="mt-1 text-sm leading-6 text-muted-foreground">{description}</p>
      </div>
      {children}
      {feedback && <div className="w-full min-w-0 basis-full">{feedback}</div>}
    </div>
  )
}
export function Confirm({
  open,
  onOpenChange,
  title,
  description,
  onConfirm,
  restoreFocus,
  confirmLabel,
  pendingLabel,
  cancelLabel,
  confirmVariant = 'destructive',
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  title: string
  description: string
  onConfirm: () => Promise<unknown>
  restoreFocus?: () => void
  confirmLabel?: string
  pendingLabel?: string
  cancelLabel?: string
  confirmVariant?: 'default' | 'destructive'
}) {
  const t = useText()
  const [pending, setPending] = useState(false),
    [error, setError] = useState<UiMessage | null>(null)
  async function run() {
    setPending(true)
    setError(null)
    try {
      await onConfirm()
      onOpenChange(false)
    } catch (e) {
      setError(asAppError(e))
    } finally {
      setPending(false)
    }
  }
  return (
    <AlertDialog.Root
      open={open}
      onOpenChange={(v) => {
        if (!pending) {
          setError(null)
          onOpenChange(v)
        }
      }}
    >
      <AlertDialog.Portal>
        <AlertDialog.Overlay className="fixed inset-0 z-50 bg-black/40" />
        <AlertDialog.Content
          className="fixed top-1/2 left-1/2 z-50 grid w-[calc(100%-2rem)] max-w-lg -translate-x-1/2 -translate-y-1/2 gap-5 rounded-xl border bg-popover p-6 shadow-lg"
          onCloseAutoFocus={
            restoreFocus
              ? (event) => {
                  event.preventDefault()
                  restoreFocus()
                }
              : undefined
          }
        >
          <div className="space-y-2">
            <AlertDialog.Title className="text-lg font-semibold">{title}</AlertDialog.Title>
            <AlertDialog.Description className="text-sm leading-6 text-muted-foreground">
              {description}
            </AlertDialog.Description>
          </div>
          <InlineError message={error} />
          <DialogFooter>
            <AlertDialog.Cancel asChild>
              <Button variant="outline" disabled={pending}>
                {cancelLabel ?? t('取消', 'Cancel')}
              </Button>
            </AlertDialog.Cancel>
            <Button variant={confirmVariant} disabled={pending} onClick={() => void run()}>
              {pending
                ? (pendingLabel ?? t('正在删除…', 'Deleting…'))
                : (confirmLabel ?? t('确认删除', 'Delete'))}
            </Button>
          </DialogFooter>
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  )
}
