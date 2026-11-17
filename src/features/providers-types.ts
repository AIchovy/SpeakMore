export type ProviderProtocol =
  | { kind: 'openai' | 'anthropic' | 'ollama' }
  | {
      kind: 'openai_compatible'
      budget_field: 'max_tokens' | 'max_completion_tokens'
      instruction_role: 'system' | 'developer'
    }
export interface ProviderRow {
  providerId: string
  revision: number
  displayName: string
  protocol: ProviderProtocol
  endpoint: string
  model: string
  local: boolean
  credentialConfigured: boolean
  destinationConfirmed: boolean
  testedAt: number | null
  testValid: boolean
  preparedThisRun: boolean
}
export interface ProvidersView {
  revision: number
  activeProviderId: string | null
  rows: ProviderRow[]
  recoveryRows: ProviderRecoveryRow[]
  totalProviders: number
  truncated: boolean
  cleanupPending: number
  busy: boolean
  operation: string | null
  cleanupError: { code: string; userMessage: string; retryable: boolean } | null
}
export interface ProviderRecoveryRow {
  recoveryId: string
  displayName: string | null
  kind: 'provider' | 'credential'
}
export type CredentialIntent = 'keep' | 'replace' | 'remove'
export function consumeCredential(intent: CredentialIntent, input: { value: string } | null) {
  const key = input?.value ?? ''
  if (input) input.value = ''
  return intent === 'replace' ? { action: intent, key } : { action: intent }
}
export const endpoints: Record<ProviderProtocol['kind'], string> = {
  openai: 'https://api.openai.com/v1/chat/completions',
  anthropic: 'https://api.anthropic.com/v1/messages',
  ollama: 'http://127.0.0.1:11434/api/chat',
  openai_compatible: '',
}
export const providerNames = {
  openai: 'OpenAI',
  anthropic: 'Anthropic',
  ollama: 'Ollama',
  openai_compatible: 'OpenAI compatible',
}
export function localOllama(kind: ProviderProtocol['kind'], endpoint: string) {
  return kind === 'ollama' && /^https?:\/\/(127\.0\.0\.1|\[::1\])(?::[0-9]+)?\//.test(endpoint)
}
