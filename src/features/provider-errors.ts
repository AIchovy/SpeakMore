import { errorMessage } from '@/lib/types'

export function providerErrorMessage(
  error: unknown,
  translate: (zh: string, en: string) => string,
): string | null {
  if (!error) return null
  return errorMessage(error, translate)
}
