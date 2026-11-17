import { Confirm } from '@/components/common'
import { useApp, useText } from '@/lib/app-state'

export function PermissionRestartPrompt() {
  const { data, permissionRestart, dismissPermissionRestart, mutate } = useApp()
  const t = useText()
  const granted = data?.permissions.accessibility === true
  return (
    <Confirm
      open={
        !!permissionRestart && !!data?.desktop && !data.session.sessionId && !data.captureSuspended
      }
      onOpenChange={(open) => {
        if (!open) dismissPermissionRestart()
      }}
      title={
        granted
          ? t('授权已开启，请重启 SpeakMore', 'Access allowed. Restart SpeakMore')
          : t('开启授权后，请重启 SpeakMore', 'Restart SpeakMore after allowing access')
      }
      description={t(
        `${granted ? '重启后将重新启用 Fn 快捷键和自动输入。' : '如果你已在系统设置中开启辅助功能，请重启以应用授权。'} 无需重新保存快捷键。请先复制需要保留的最近结果，未保存到历史的文字会被清空。`,
        `${granted ? 'Restart to reinitialize the Fn shortcut and automatic insertion.' : 'If you enabled Accessibility in System Settings, restart to apply access.'} You do not need to save your shortcut again. Copy any recent results you need first; text not saved to history will be cleared.`,
      )}
      confirmLabel={t('现在重启', 'Restart now')}
      pendingLabel={t('正在重启…', 'Restarting…')}
      cancelLabel={t('稍后', 'Later')}
      confirmVariant="default"
      onConfirm={() => mutate('restart_to_apply_permissions', {})}
    />
  )
}
