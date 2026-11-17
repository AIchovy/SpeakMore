import type { WritingRule } from '@/features/modes'
import modelCatalog from '../../resources/model-catalog.payload.json'
import type { DictionaryInput, Snapshot, SettingsPatch, DictionaryEntry } from './types'

// In-memory browser workbench only. Never persists transcript data or imitates ASR.
export const createPreview = () => {
  const data: Snapshot = {
    revision: 1,
    desktop: false,
    canStart: false,
    captureSuspended: false,
    shortcutNeedsRelease: false,
    readinessReason: '请在桌面应用中安装语音模型',
    readinessCode: 'model_required',
    readinessDetailCode: null,
    readinessAction: 'models',
    permissions: { microphone: 'unavailable', accessibility: false, requestingMicrophone: false },
    usageToday: 0,
    dictionary: [],
    recentResults: [],
    worker: {
      phase: 'dormant',
      modelId: null,
      backend: 'cpu',
      consecutiveFailures: 0,
      sessionId: null,
    },
    modelOperation: null,
    modelTargetId: null,
    modelName: null,
    audioLevel: 0,
    lastError: null,
    storageWarning: null,
    session: {
      sessionId: null,
      revision: 0,
      phase: 'idle',
      elapsedMs: 0,
      maxDurationMs: 300_000,
      mode: null,
    },
    settings: {
      revision: 1,
      language: 'auto',
      uiLocale: 'zh-CN',
      theme: 'system',
      motionPolicy: 'system',
      sidebarCollapsed: false,
      deliveryMode: 'paste',
      polishEnabled: false,
      polishMode: 'standard',
      customPrompt: '',
      processingPolicy: 'local_only',
      historyEnabled: false,
      historyLimit: 500,
      historyDays: 30,
      inputDeviceId: null,
      activeModelId: null,
      activeProviderId: null,
    },
  }
  let writingRules: WritingRule[] = []
  const fail = (code: string, userMessage: string): never => {
    throw { code, userMessage, retryable: false }
  }
  function revision(expected: unknown) {
    if (expected !== data.settings.revision) fail('RevisionConflict', '设置已改变，请刷新后重试。')
  }
  return async (command: string, args: Record<string, unknown> = {}): Promise<unknown> => {
    switch (command) {
      case 'overview_activity': {
        const fixture =
          import.meta.env.DEV && typeof window !== 'undefined'
            ? new URLSearchParams(window.location.search).get('activity-fixtures')
            : null
        if (fixture === 'error') return fail('StorageUnavailable', '使用统计暂时无法读取。')
        if (fixture)
          return (await import('@/features/overview-activity.preview')).activityFixture(
            args.day as string | null,
            fixture === 'empty',
          )
        return { days: [], dictations: [], meetings: [], captions: [] }
      }
      case 'enable_polishing':
        return fail('DesktopRequired', '请在桌面应用中配置并启用 AI 文字整理。')
      case 'get_providers':
        return {
          revision: data.settings.revision,
          activeProviderId: null,
          rows: [],
          recoveryRows: [],
          totalProviders: 0,
          truncated: false,
          cleanupPending: 0,
          busy: false,
          operation: null,
          cleanupError: null,
        }
      case 'save_provider':
      case 'test_provider':
      case 'cancel_provider_test':
      case 'select_provider':
      case 'delete_provider':
      case 'retry_provider_cleanup':
      case 'discard_invalid_provider':
        return fail('DesktopRequired', 'AI 服务和系统钥匙串仅在桌面应用中可用。')
      case 'get_storage_status':
        return { error: null, cleanupPending: false, canReveal: false }
      case 'reveal_recovery_data':
      case 'restart_after_storage_error':
        return fail('DesktopRequired', '数据恢复仅在桌面应用中可用。')
      case 'list_writing_rules':
        return { revision: data.settings.revision, items: writingRules.map((r) => ({ ...r })) }
      case 'upsert_writing_rule': {
        revision(args.expectedRevision)
        const input = args.rule as WritingRule
        const rule = { ...input, prompt: input.prompt.trim(), target: input.target.trim() }
        if (!rule.prompt || new TextEncoder().encode(rule.prompt).length > 4096)
          fail('InvalidInput', '请填写整理要求，最多 4096 字节。')
        if (rule.kind === 'website') {
          let url: URL
          try {
            url = new URL(rule.target.includes('://') ? rule.target : 'https://' + rule.target)
          } catch {
            return fail('InvalidInput', '请输入有效的网站地址。')
          }
          if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password)
            return fail('InvalidInput', '请输入有效的网站地址。')
          rule.target = url.host.toLowerCase() + url.pathname.replace(/\/+$/, '')
        }
        if (
          writingRules.some(
            (r) => r.id !== rule.id && r.kind === rule.kind && r.target === rule.target,
          )
        )
          fail('InvalidInput', '这个应用或网站已有规则，请编辑已有规则。')
        writingRules = [...writingRules.filter((r) => r.id !== rule.id), rule]
        data.settings.revision++
        data.revision++
        return { revision: data.settings.revision, items: writingRules.map((r) => ({ ...r })) }
      }
      case 'delete_writing_rule':
        revision(args.expectedRevision)
        writingRules = writingRules.filter((r) => r.id !== args.id)
        data.settings.revision++
        data.revision++
        return { revision: data.settings.revision, items: writingRules.map((r) => ({ ...r })) }
      case 'list_app_rules':
        return { revision: data.settings.revision, items: [] }
      case 'choose_app_for_rule':
      case 'upsert_app_rule':
      case 'delete_app_rule':
        return fail('DesktopRequired', '请在 macOS 应用中选择和管理应用规则。')
      case 'get_recording_controls':
        return {
          revision: 1,
          preferences: {
            pushToTalk: { accelerator: 'Fn', enabled: true },
            cancel: { accelerator: 'Control+Alt+Escape', enabled: false },
          },
          bindings: [
            {
              action: 'push_to_talk',
              accelerator: 'Fn',
              enabled: true,
              registered: false,
              error: null,
            },
            {
              action: 'cancel',
              accelerator: 'Control+Alt+Escape',
              enabled: false,
              registered: false,
              error: null,
            },
          ],
          hudAvailable: false,
          error: null,
          serviceError: null,
        }
      case 'list_models':
        return {
          revision: 1,
          busy: false,
          activeJobId: null,
          jobModelId: null,
          jobKind: null,
          cancellable: false,
          models: modelCatalog.models.map((model, index) => ({
            modelId: 'version' in model ? `${model.model_id}-preview-${index}` : model.model_id,
            familyModelId: model.model_id,
            version: 'version' in model ? String(model.version) : null,
            displayName: model.display_name,
            family: model.family,
            dictionaryPrompt: model.capabilities.dictionary_prompt,
            licenseLabel: model.artifacts.find((file) => file.role === 'model')?.license ?? '',
            languages: [...model.languages],
            quantization: model.quantization,
            sizeBytes: model.artifacts.reduce((n, file) => n + file.size_bytes, 0),
            state: 'absent',
            receivedBytes: 0,
            attempt: 0,
            error: null,
            errorMessage: null,
            supported: true,
            artifacts: model.artifacts.map((file) => ({
              filename: file.filename,
              sizeBytes: file.size_bytes,
              role: file.role,
            })),
          })),
        }
      case 'get_snapshot':
        return structuredClone(data)
      case 'update_settings': {
        revision(args.expectedRevision)
        const patch = args.patch as SettingsPatch
        if (patch.polishEnabled) fail('ProviderNotReady', '请先配置并测试 AI 服务。')
        Object.assign(data.settings, patch, { revision: data.settings.revision + 1 })
        data.revision++
        return structuredClone(data.settings)
      }
      case 'upsert_dictionary': {
        revision(args.expectedRevision)
        const input = args.input as DictionaryInput
        const term = input.term.trim().normalize('NFC')
        if (!term || [...term].length > 40 || /[\p{Cc}]/u.test(term))
          fail('InvalidInput', '词条需要 1–40 个字符，不能包含控制字符。')
        if (
          data.dictionary.some(
            (e) =>
              e.entryId !== input.entryId &&
              e.locale === input.locale &&
              e.term.toLocaleLowerCase() === term.toLocaleLowerCase(),
          )
        )
          fail('TermAlreadyExists', '该语言下已存在这个词条。')
        const current = data.dictionary.find((e) => e.entryId === input.entryId)
        if (input.entryId && !current) fail('NotFound', '词条已被删除，请刷新列表。')
        if (!current && data.dictionary.length >= 200)
          fail('InvalidInput', '词典最多保存 200 个词条。')
        const entry: DictionaryEntry = {
          ...input,
          term,
          entryId: current?.entryId ?? crypto.randomUUID(),
          createdAt: current?.createdAt ?? Date.now(),
          updatedAt: Date.now(),
        }
        data.dictionary = current
          ? data.dictionary.map((e) => (e.entryId === entry.entryId ? entry : e))
          : [...data.dictionary, entry]
        data.settings.revision++
        data.revision++
        return structuredClone(entry)
      }
      case 'delete_dictionary':
        revision(args.expectedRevision)
        data.dictionary = data.dictionary.filter((e) => e.entryId !== args.entryId)
        data.settings.revision++
        data.revision++
        return null
      case 'list_history':
        return { items: [], nextCursor: null }
      case 'clear_history':
        return 0
      case 'delete_history':
        return 0
      default:
        return fail('DesktopRequired', '此操作需要桌面应用。浏览器预览不会启动录音或生成转写。')
    }
  }
}
