import type { Settings } from './types'

const labels: Record<Settings['polishMode'], [string, string]> = {
  standard: ['标准整理', 'Standard'],
  email: ['邮件正文', 'Email'],
  code_comment: ['代码注释', 'Code comments'],
  custom: ['自定义', 'Custom'],
}

export function polishModeLabel(
  mode: Settings['polishMode'],
  text: (zh: string, en: string) => string,
) {
  return text(...labels[mode])
}
