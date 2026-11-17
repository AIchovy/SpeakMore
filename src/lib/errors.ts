import type { AppError } from './types'

export type Translate = (zh: string, en: string) => string
export type UiMessage = string | AppError

const english: Record<string, string> = {
  Busy: 'Another operation or dictation is still in progress. Please try again shortly.',
  RevisionConflict: 'Settings changed elsewhere. Close this form and reload before trying again.',
  InvalidInput: 'Check the entered values and limits, then try again.',
  StorageUnavailable: 'Local data could not be saved. Check available disk space and try again.',
  DesktopRequired: 'Open the macOS app to perform this action.',
  ProviderUnavailable:
    'The operation did not finish. Reopen settings to check the saved configuration.',
  ProviderRecoveryExpired:
    'This review expired or the configuration changed. Close the dialog and refresh the service list before trying again.',
  ProviderNotTested: 'Test this service successfully before selecting it.',
  ProviderEndpointInvalid:
    'Enter a full endpoint URL. Remote services require HTTPS without credentials or query parameters in the URL.',
  ProviderModelInvalid: 'Enter an exact model ID available to this service.',
  ProviderLocalOnly: 'Cloud text processing is off. Enable it before testing a remote service.',
  DestinationConsentRequired:
    'Confirm the destination address before sending text to this service.',
  ProviderTestCancelled:
    'Connection test cancelled. A test sentence already sent cannot be recalled.',
  ProviderDeadline:
    'The service did not finish within 2.5 seconds. Check that the model is ready and test again.',
  ProviderBusy: 'The AI service is handling another operation. Try again shortly.',
  ProviderUnauthorized: 'The service rejected authentication. Check the key and model permissions.',
  ProviderRateLimited: 'The service limit or quota was reached. Check your provider account.',
  ProviderNetworkUnavailable:
    'Could not connect securely. Check the endpoint, network or local service.',
  ProviderRefused: 'The service declined the test sentence. Its response was not accepted.',
  ProviderResponseInvalid:
    'The service did not return complete text. Check the model and request settings.',
  CredentialBusy: 'The system keychain is still handling an earlier operation. Try again shortly.',
  CredentialMissing: 'No saved key is available. Save a key and try again.',
  CredentialInvalid:
    'The key is invalid, or this service does not accept a key. Check the key settings.',
  CredentialAmbiguous: 'Multiple matching credentials were found. The key could not be selected.',
  CredentialCancelled: 'System keychain authorization was cancelled.',
  CredentialDenied: 'The system keychain denied access. Check authorization and try again.',
  CredentialInteractionRequired:
    'The keychain needs to be unlocked or authorized. Complete the system prompt and try again.',
  CredentialUnavailable: 'The system keychain is unavailable. Check its status and try again.',
  CredentialDeleteUnconfirmed: 'Key removal has not been confirmed. Retry cleanup.',
  CredentialUnsupported: 'System credential storage is unavailable on this platform.',
  CredentialRebindRequired:
    'The service type or destination changed. Enter a new key or explicitly remove the old key.',
  CredentialCleanupRequired: 'Complete pending key cleanup before replacing another key.',
  CredentialInUse: 'A saved configuration still uses this key. It cannot be removed yet.',

  Unknown: 'The operation could not finish. Please try again.',
  Initializing: 'Preparing local services. Please wait.',
  InvalidRequest: 'The request is no longer valid. Refresh and try again.',
  Forbidden: 'This window cannot perform that action. Open the main SpeakMore window.',
  NotFound: 'This item was removed. Refresh the list and select it again.',
  NotReady: 'The microphone or model is not ready. Check Overview and settings.',
  SessionUnavailable: 'Local dictation is unavailable. Restart SpeakMore and try again.',
  SessionInterrupted:
    'Dictation was interrupted by a system change. Start again when the system is ready.',
  KeyReleaseRequired: 'Release the shortcut before starting another dictation.',
  ControlsUnavailable: 'Keyboard controls are unavailable. Restart SpeakMore and try again.',
  FnPermissionRequired:
    'Allow SpeakMore in System Settings → Privacy & Security → Accessibility, then restart the app. You do not need to save your shortcut again.',
  HudUnavailable: 'The recording bar is unavailable. You can dictate from Overview.',
  CaptionWindowUnavailable: 'The caption panel is unavailable. Restart SpeakMore and try again.',
  ShortcutConflict:
    'This shortcut is unavailable or conflicts with another action. Choose another combination.',
  ShortcutCleanupFailed:
    'Some previous shortcuts could not be released. Retry cleanup or restart SpeakMore.',
  PermissionServiceUnavailable:
    'System permissions could not be checked. Restart SpeakMore and try again.',
  SystemSettingsUnavailable:
    'Open System Settings, then Privacy & Security, and check the required permission.',
  AccessibilityRequired:
    'Accessibility access is required for this action. Check System Settings; results can still be copied.',
  MicrophonePermissionRevoked:
    'Microphone access changed and dictation was cancelled. Check System Settings.',
  NoInputDevice: 'No microphone was found. Connect one, then refresh the device list.',
  DeviceUnavailable: 'The microphone is disconnected or unavailable. Choose a device again.',
  UnsupportedAudioFormat: 'This microphone’s audio format is not supported. Choose another device.',
  InvalidChannel: 'The selected microphone channel is unavailable. Check audio settings.',
  MicrophonePermissionDenied: 'Allow SpeakMore to use your microphone in System Settings.',
  DeviceBusy: 'The microphone is busy. Try again shortly.',
  AudioBusy: 'The microphone is busy. Try again shortly.',
  MicrophonePrepareTimeout:
    'The microphone took too long to start. Check access and the device, then try again.',
  AudioIncomplete:
    'Recording was interrupted or incomplete. Check the microphone and record again.',
  InvalidAudioSamples:
    'The recording contains invalid audio. Check the microphone and record again.',
  AudioTooShort: 'The recording is too short. Record a complete phrase and try again.',
  AudioDrainTimeout:
    'The recording could not be completed in time. Check the microphone and record again.',
  AudioResamplingFailed:
    'The recording could not be processed. Check the microphone format and record again.',
  AudioMemoryUnavailable: 'There is not enough available memory. Close some apps and try again.',
  AudioThreadUnavailable: 'The microphone could not be checked. Restart SpeakMore and try again.',
  WorkerUnavailable: 'The speech engine is unavailable. Reload your model or restart SpeakMore.',
  WorkerProtocolError:
    'The speech engine returned an invalid response. Reload your model and try again.',
  WorkerStartTimeout: 'The speech engine took too long to start. Reload your model and try again.',
  ModelLoadTimeout: 'The model took too long to load. Reload it or select another model.',
  AudioTransferTimeout:
    'The recording could not reach the speech engine in time. Reload your model and record again.',
  AsrTimeout: 'Transcription took too long. Reload your model and try again.',
  ModelUnavailable: 'The model is unavailable. Reload it or select another installed model.',
  ModelLoadFailed: 'The model could not be loaded. Check the selected model and try again.',
  NoSpeech: 'No clear speech was detected. Check your microphone and record again.',
  ResultTooLong: 'The transcript exceeds the text limit. Record shorter sections.',
  AsrFailed: 'Transcription failed. Reload your model and record again.',
  UntrustedCatalog: 'The model catalog signature is invalid. Update SpeakMore.',
  InvalidCatalog: 'The model catalog is invalid. Update SpeakMore.',
  UnsupportedModel: 'This model is not supported by the current system or speech engine.',
  InUse: 'This model is in use. Select another model or unload it before removing its files.',
  ModelInUse: 'Unload the current model before removing its files.',
  Cancelled: 'The operation was cancelled.',
  Committing: 'Installation is being finalized and can no longer be cancelled.',
  InsufficientSpace: 'There is not enough disk space. Free some space and try again.',
  Network: 'The connection was interrupted or timed out. Check your network and try again.',
  Server: 'The download service is temporarily unavailable. Try again shortly.',
  HttpStatus:
    'The download source cannot provide this model file. Update SpeakMore and try again, or import it from this Mac.',
  SizeMismatch: 'The file size did not match. Previous model files have been kept.',
  HashMismatch: 'File verification failed. Previous model files have been kept.',
  Storage:
    'The model folder could not be accessed safely. Check disk space and permissions, then try again.',
  Registration:
    'Model files are in place but installation could not be registered. Refresh the model library.',
  NotInstalled: 'The model is missing or incomplete. Download or import it again.',
  ImportSelection:
    'Select every listed file with its original filename. Do not select folders, archives or extra files.',
  ModelPickerUnavailable: 'The selected model files could not be read. Select them again.',
  AppPickerBusy: 'Finish or cancel the open file selection dialog first.',
  AppPickerUnavailable:
    'The selected application could not be read. Choose a valid .app application.',
  AppSelectionExpired: 'The application selection expired. Select the application again.',
  ExportBusy: 'Another export is in progress. Finish or cancel it first.',
  ExportFailed:
    'The export could not be saved. Check disk space and folder permissions, then try again.',
  SaveDialogUnavailable: 'The save dialog could not be opened. Try again.',
  ResultExpired: 'This result expired or was deleted. Check saved history if it was enabled.',
  ClipboardBusy: 'The previous copy is still running. Try again shortly.',
  CopyExpired: 'The copy request expired or was cancelled before writing.',
  CopyUncertain:
    'The copy could not be confirmed. Check the clipboard; your text is still available.',
  ClipboardChanged: 'Another operation changed the clipboard. Check it before copying again.',
  ClipboardUnavailable: 'The clipboard is unavailable. Your text is still available.',
  InvalidCopyText: 'The text is empty or exceeds the copy limit.',
  HistoryNotSaved:
    'The result was not saved to history. You can still copy it from recent results.',
  LocalCleanupIncomplete: 'Local cleanup did not finish. You can still copy the result.',
  UsageNotSaved: 'Local usage could not be saved. You can still copy the text from recent results.',
  RefinementNotSaved:
    'The refined result was not saved to history. You can still copy it from recent results.',
  DeliveryStatusNotSaved:
    'The delivery status could not be saved. You can still copy the text from recent results.',
  DeliveryReceiptNotSaved:
    'The delivery status could not be saved. The result is kept; check the target application.',
  ProviderNotReady: 'Configure and test an AI service before enabling refinement.',
  RecoveryUnavailable: 'There is no startup storage issue to recover. Reopen SpeakMore if needed.',
  PlatformUnsupported: 'This action is available in the macOS app.',
  UnsupportedPlatform: 'This action is available in the macOS app.',
  AuditUnavailable: 'The native verification tool is unavailable. Reopen the verification app.',
  TermAlreadyExists:
    'This term already exists for the selected language. Edit the existing entry or choose another language.',
  HistoryChanged:
    'The selected history records changed or were deleted. Refresh and select them again.',
  DeliveryStopped: 'Automatic input stopped. You can still copy the text.',
  ModifiersHeld: 'Modifier keys are still held. Release them and paste the text manually.',
  InputStateUnavailable:
    'Keyboard state or input permission could not be confirmed. No automatic input was attempted; you can still copy the text.',
  ShortcutEditExpired: 'Shortcut editing ended. Reopen the shortcut settings to try again.',
  DatabaseCorrupt:
    'Local data failed its integrity check. Original files have been kept; read the recovery instructions.',
  DatabaseBackupFailed:
    'The backup could not be completed, so the upgrade stopped. Check disk space and folder permissions.',
  DatabaseMigrationFailed:
    'The local data upgrade did not finish. Original files have been kept and dictation has not started.',
  DatabaseRecoveryRequired:
    'The main database is missing or empty, but recovery files exist. Check the backup before continuing.',
  NewerDatabase:
    'This data was created by a newer version. Open it with the same or a newer SpeakMore version.',
  UnknownDatabase: 'The local data format could not be recognized. Original files have been kept.',
}

const genericZh = '操作没有完成，请重试。'

// Native errors cross IPC as data. Never stringify unexpected exceptions or
// expose an unknown native/provider response as user-facing text.
export function asAppError(error: unknown): AppError {
  if (typeof error !== 'object' || error === null || !Object.hasOwn(error, 'code'))
    return { code: 'Unknown', userMessage: genericZh, retryable: false }
  const value = error as Partial<AppError>
  if (typeof value.code !== 'string' || !Object.hasOwn(english, value.code))
    return { code: 'Unknown', userMessage: genericZh, retryable: false }
  return {
    code: value.code,
    userMessage:
      typeof value.userMessage === 'string' &&
      value.userMessage.trim() &&
      value.userMessage.length <= 4096
        ? value.userMessage
        : genericZh,
    retryable: value.retryable === true,
  }
}

export function errorMessage(error: unknown, translate: Translate): string {
  const value = asAppError(error)
  return translate(value.userMessage, english[value.code])
}
