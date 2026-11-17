import { describe, expect, it, vi } from 'vitest'
import { renderToStaticMarkup } from 'react-dom/server'
import { ImportRequirements } from './model-import'
import { createPreview } from '@/lib/preview'
import type { ModelsSnapshot } from '@/lib/types'
vi.mock('@/lib/app-state', () => ({ useText: () => (zh: string) => zh, call: vi.fn() }))
describe('local model import instructions', () => {
  it('lists every required artifact for each engine, including tokens and VAD, with explicit copy semantics', async () => {
    const catalog = (await createPreview()('list_models')) as ModelsSnapshot
    for (const model of catalog.models) {
      const html = renderToStaticMarkup(<ImportRequirements artifacts={model.artifacts} />)
      for (const file of model.artifacts) expect(html).toContain(file.filename)
      expect(html).toContain('不会联网、移动原文件或切换当前模型')
      expect(html).toContain('选择解压后的文件')
      expect(html).not.toContain('https://')
    }
  })
})
