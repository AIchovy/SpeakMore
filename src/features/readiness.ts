import type { ReadinessCode, Snapshot } from '@/lib/types'

const english: Record<ReadinessCode, string> = {
  ready: 'Ready to dictate.',
  shortcut_editing: 'Finish editing your shortcuts before dictating.',
  shutting_down: 'SpeakMore is shutting down.',
  system_suspended: 'Dictation is paused while the system sleeps or your user session is inactive.',
  recovering_interruption: 'Finishing interrupted dictation, then checking your model again.',
  dictation_busy: 'The current dictation has not finished yet.',
  storage_unavailable: 'Local data is unavailable. Check storage and restart SpeakMore.',
  updating_configuration: 'Updating local settings.',
  preparing_model: 'Preparing your speech model.',
  reading_settings: 'Reading local settings.',
  model_files_busy: 'Processing model files. Please wait.',
  model_service_unavailable:
    'The model library is unavailable. Check local storage and restart SpeakMore.',
  engine_unavailable: 'The speech engine is unavailable. Reload your model in settings.',
  engine_preparing: 'Preparing the speech engine.',
  model_required: 'Select an installed speech model in settings.',
  model_configuration_mismatch:
    'The loaded model does not match your settings. Select the model again.',
  microphone_prompt_pending: 'Choose whether to allow microphone access in the system prompt.',
  microphone_not_granted: 'Allow SpeakMore to use your microphone before dictating.',
  microphone_denied: 'Microphone access is off. Enable it in System Settings.',
  microphone_restricted:
    'Microphone access is restricted. Check device management or parental controls.',
  microphone_status_unavailable:
    'Microphone access could not be checked. Check System Settings or restart SpeakMore.',
  input_device_unavailable:
    'The selected microphone is unavailable. Check its connection or choose another device.',
  checking_input_device:
    'Checking the selected microphone. If this takes too long, check its connection and restart SpeakMore.',
}

const audioDetails: Record<string, string> = {
  NoInputDevice: 'No microphone was found. Connect one, then refresh the device list.',
  DeviceUnavailable: 'The microphone is disconnected or unavailable. Choose a device again.',
  UnsupportedAudioFormat: 'This microphone’s audio format is not supported. Choose another device.',
  InvalidChannel: 'The selected microphone channel is unavailable. Check your audio settings.',
  MicrophonePermissionDenied: 'Allow SpeakMore to use your microphone in System Settings.',
  DeviceBusy: 'The microphone is busy. Try again shortly.',
  AudioBusy: 'The microphone is busy. Try again shortly.',
  MicrophonePrepareTimeout:
    'The microphone took too long to start. Check access and the device, then try again.',
  AudioMemoryUnavailable: 'There is not enough available memory. Close some apps and try again.',
  AudioThreadUnavailable: 'The microphone could not be checked. Restart SpeakMore and try again.',
}

export function readinessMessage(
  snapshot: Pick<Snapshot, 'readinessCode' | 'readinessDetailCode' | 'readinessReason'>,
  translate: (zh: string, en: string) => string,
): string {
  const code = snapshot.readinessCode
  if (!Object.hasOwn(english, code)) {
    // Older/unknown native state: never show an untranslated diagnostic or
    // infer readiness from a UI string. canStart remains the native gate.
    return translate(
      '听写暂不可用，请检查设置或重启应用。',
      'Dictation is unavailable. Check settings or restart SpeakMore.',
    )
  }
  const detail = snapshot.readinessDetailCode
  const message =
    code === 'input_device_unavailable' && detail && Object.hasOwn(audioDetails, detail)
      ? audioDetails[detail]
      : english[code]
  return translate(snapshot.readinessReason, message)
}
