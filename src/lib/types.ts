export interface AppError {
  code: string
  userMessage: string
  retryable: boolean
}
export interface RecordingStatus {
  revision: number
  session: Snapshot['session']
  audioLevel: number
  uiLocale: string
  motionPolicy: string
  preview: boolean
  notice: {
    sessionId: string
    hasResult: boolean
    deliveryStatus?: string | null
    deliveryReason?: string | null
    error: AppError | null
  } | null
}
export interface RecordingControls {
  revision: number
  preferences: ShortcutPreferences
  bindings: (ShortcutBinding & {
    action: ShortcutAction
    registered: boolean
    error: AppError | null
  })[]
  hudAvailable: boolean
  error: AppError | null
  serviceError: AppError | null
}
export type ShortcutAction = 'push_to_talk' | 'cancel'
export interface ShortcutBinding {
  accelerator: string
  enabled: boolean
}
export interface ShortcutPreferences {
  pushToTalk: ShortcutBinding
  cancel: ShortcutBinding
}
export interface Settings {
  revision: number
  language: 'auto' | 'zh' | 'en'
  uiLocale: 'zh-CN' | 'en'
  theme: 'system' | 'light' | 'dark'
  motionPolicy: 'system' | 'reduced'
  sidebarCollapsed: boolean
  deliveryMode: 'paste' | 'manual'
  polishEnabled: boolean
  polishMode: 'standard' | 'email' | 'code_comment' | 'custom'
  customPrompt: string
  processingPolicy: 'local_only' | 'allow_cloud'
  historyEnabled: boolean
  historyLimit: number
  historyDays: number
  inputDeviceId: string | null
  activeModelId: string | null
  activeProviderId: string | null
}
export interface InputDevice {
  id: string
  name: string
  isDefault: boolean
  configuration: {
    sampleRate: number
    channels: number
    sampleFormat: 'f32' | 'i16' | 'u16'
  } | null
}
export type SettingsPatch = Partial<
  Omit<Settings, 'revision' | 'inputDeviceId' | 'activeModelId' | 'activeProviderId'>
>
export interface DictionaryEntry {
  entryId: string
  term: string
  locale: 'all' | 'zh' | 'en'
  enabled: boolean
  pinned: boolean
  createdAt: number
  updatedAt: number
}
export type DictionaryInput = Omit<DictionaryEntry, 'entryId' | 'createdAt' | 'updatedAt'> & {
  entryId?: string
}
export type Phase =
  | 'idle'
  | 'preparing'
  | 'recording'
  | 'finalizing_audio'
  | 'transcribing'
  | 'polishing'
  | 'delivery_check'
  | 'submitting'
  | 'cancelling'
  | 'finishing'
export interface SessionSnapshot {
  sessionId: string | null
  revision: number
  phase: Phase
  elapsedMs: number
  maxDurationMs: number
  mode: 'hold' | 'toggle' | null
}
export interface ResultRecord {
  sessionId: string
  rawText: string
  polishedText: string | null
  modelId: string
  modelSha256: string
  dictionaryRevision: number
  language: string
  durationMs: number
  asrMs: number
  needsReview: boolean
  polishMs: number | null
  polishOutcome: string
  polishReason?: string | null
  deliveryStatus: string
  deliveryReason: string | null
  appIdentity: string | null
  createdAt: number
}
export type DeliveryUpdate = Pick<ResultRecord, 'sessionId' | 'deliveryStatus' | 'deliveryReason'>
export interface AppRule {
  platform: 'macos'
  appIdentity: string
  displayName: string
  manualOnly: boolean
  historyExcluded: boolean
}
export interface AppRulesSnapshot {
  revision: number
  items: AppRule[]
}
export interface AppChoice {
  selectionId: string
  appIdentity: string
  displayName: string
}
export type ReadinessCode =
  | 'ready'
  | 'shortcut_editing'
  | 'shutting_down'
  | 'system_suspended'
  | 'recovering_interruption'
  | 'dictation_busy'
  | 'storage_unavailable'
  | 'updating_configuration'
  | 'preparing_model'
  | 'reading_settings'
  | 'model_files_busy'
  | 'model_service_unavailable'
  | 'engine_unavailable'
  | 'engine_preparing'
  | 'model_required'
  | 'model_configuration_mismatch'
  | 'microphone_prompt_pending'
  | 'microphone_not_granted'
  | 'microphone_denied'
  | 'microphone_restricted'
  | 'microphone_status_unavailable'
  | 'input_device_unavailable'
  | 'checking_input_device'

export interface Snapshot {
  writingContext?: {
    appIdentity: string | null
    appName: string | null
    windowTitle: string | null
    inputTitle: string | null
    websiteUrl: string | null
  } | null
  writingRule?: string | null
  revision: number
  settings: Settings
  session: SessionSnapshot
  dictionary: DictionaryEntry[]
  recentResults: ResultRecord[]
  usageToday: number
  canStart: boolean
  captureSuspended: boolean
  shortcutNeedsRelease: boolean
  readinessReason: string
  readinessCode: ReadinessCode
  readinessDetailCode: string | null
  readinessAction: 'none' | 'wait' | 'models' | 'microphone' | 'device' | 'storage'
  permissions: {
    microphone: 'not_determined' | 'restricted' | 'denied' | 'authorized' | 'unavailable'
    accessibility: boolean
    requestingMicrophone: boolean
  }
  desktop: boolean
  worker: {
    phase:
      | 'dormant'
      | 'starting'
      | 'loading'
      | 'ready'
      | 'transcribing'
      | 'cancelling'
      | 'recovering'
      | 'unavailable'
      | 'paused'
      | 'shutting_down'
      | 'stopped'
    modelId: string | null
    backend: 'cpu' | 'metal' | 'vulkan'
    consecutiveFailures: number
    sessionId: string | null
  }
  modelOperation: 'verifying' | 'loading' | 'saving' | 'unloading' | 'recovering' | null
  modelTargetId: string | null
  modelName: string | null
  audioLevel: number
  lastError: AppError | null
  storageWarning: AppError | null
}
export interface HistoryPage {
  items: ResultRecord[]
  nextCursor: string | null
}
export { asAppError, errorMessage, type UiMessage } from './errors'

export type ModelInstallState =
  | 'checking'
  | 'absent'
  | 'downloading'
  | 'importing'
  | 'retrying'
  | 'verifying'
  | 'installing'
  | 'installed'
  | 'deleting'
  | 'invalid'
  | 'download_failed'
  | 'import_failed'
export interface ModelRow {
  modelId: string
  familyModelId: string
  version: string | null
  displayName: string
  family: string
  dictionaryPrompt: boolean
  licenseLabel: string
  languages: string[]
  quantization: string
  sizeBytes: number
  state: ModelInstallState
  receivedBytes: number
  attempt: number
  error: string | null
  errorMessage: string | null
  supported: boolean
  artifacts: { filename: string; sizeBytes: number; role: string }[]
}
export interface ModelsSnapshot {
  revision: number
  models: ModelRow[]
  busy: boolean
  activeJobId: string | null
  jobModelId: string | null
  jobKind: 'download' | 'import' | 'delete' | null
  cancellable: boolean
}
