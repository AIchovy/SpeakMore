import { asAppError, type UiMessage } from '@/lib/errors'
import { useEffect, useRef, useState } from 'react'
import { call, useText } from '@/lib/app-state'
import { type ModelRow } from '@/lib/types'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { InlineError } from '@/components/common'
import { FolderIcon } from '@/components/icons'

export function ImportRequirements({ artifacts }: { artifacts: ModelRow['artifacts'] }) {
  const t = useText()
  return (
    <div className="space-y-4">
      <p className="text-sm leading-6 text-muted-foreground">
        {t(
          '将下列文件放到同一文件夹，接着一次选中它们。保持原文件名，选择解压后的文件。',
          'Place these files in one folder, then select them together. Keep their original names and extract any archives first.',
        )}
      </p>
      <ul className="divide-y rounded-lg border bg-muted/30 px-4">
        {artifacts.map((file) => (
          <li key={file.filename} className="flex items-start justify-between gap-4 py-3 text-xs">
            <span className="min-w-0 break-all font-mono">{file.filename}</span>
            <span className="shrink-0 tabular-nums text-muted-foreground">
              {(file.sizeBytes / 1_000_000).toFixed(1)} MB
            </span>
          </li>
        ))}
      </ul>
      <p className="text-xs leading-5 text-muted-foreground">
        {t(
          '导入只复制文件，不会联网、移动原文件或切换当前模型。校验通过后，再选择“使用模型”。',
          'Import copies files locally without network access, moving originals, or switching your current model. Choose Use model after verification.',
        )}
      </p>
    </div>
  )
}

export function ImportModelDialog({
  model,
  close,
  started,
  restoreFocus,
}: {
  model: ModelRow | null
  close: () => void
  started: () => void
  restoreFocus: () => void
}) {
  const t = useText()
  const [busy, setBusy] = useState(false),
    [error, setError] = useState<UiMessage | null>(null)
  const picker = useRef<HTMLButtonElement | null>(null),
    returnToPicker = useRef(false),
    mounted = useRef(true)
  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])
  useEffect(() => {
    setError(null)
  }, [model?.modelId])
  useEffect(() => {
    if (!busy && model && returnToPicker.current) {
      returnToPicker.current = false
      picker.current?.focus()
    }
  }, [busy, model])
  async function choose() {
    if (!model || busy) return
    setBusy(true)
    setError(null)
    try {
      const id = await call<string | null>('import_model', { modelId: model.modelId })
      if (!mounted.current) return
      if (id) {
        started()
        close()
      } else returnToPicker.current = true
    } catch (e) {
      if (mounted.current) {
        setError(asAppError(e))
        returnToPicker.current = true
      }
    } finally {
      if (mounted.current) setBusy(false)
    }
  }
  return (
    <Dialog
      open={!!model}
      onOpenChange={(open) => {
        if (!open && !busy) close()
      }}
    >
      <DialogContent
        className="max-w-lg"
        closeLabel={t('关闭', 'Close')}
        onCloseAutoFocus={(event) => {
          event.preventDefault()
          restoreFocus()
        }}
      >
        <DialogHeader>
          <DialogTitle>
            {t(
              `导入 ${[model?.displayName, model?.version].filter(Boolean).join(' · ') || '模型'}`,
              `Import ${[model?.displayName, model?.version].filter(Boolean).join(' · ') || 'model'}`,
            )}
          </DialogTitle>
          <DialogDescription>
            {t(
              '使用已下载到这台 Mac 的完整模型文件。',
              'Use complete model files already saved on this Mac.',
            )}
          </DialogDescription>
        </DialogHeader>
        {model && <ImportRequirements artifacts={model.artifacts} />}
        <InlineError message={error} />
        <DialogFooter>
          <Button variant="outline" disabled={busy} onClick={close}>
            {t('取消', 'Cancel')}
          </Button>
          <Button ref={picker} disabled={busy} onClick={() => void choose()}>
            <FolderIcon aria-hidden />
            {busy
              ? t('正在读取所选文件…', 'Reading selected files…')
              : t('选择文件并导入', 'Choose files and import')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
