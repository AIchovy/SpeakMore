import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderToStaticMarkup } from 'react-dom/server'
import { MemoryRouter } from 'react-router-dom'
import type { Snapshot } from '@/lib/types'
import { createPreview } from '@/lib/preview'
import { Modes } from './modes'

let data: Snapshot
vi.mock('@/lib/app-state', () => ({
  useApp: () => ({ data, pending: false, save: vi.fn(), mutate: vi.fn() }),
  useText: () => (zh: string) => zh,
}))
beforeEach(async () => {
  data = (await createPreview()('get_snapshot')) as Snapshot
})
function markup() {
  return renderToStaticMarkup(
    <MemoryRouter>
      <Modes />
    </MemoryRouter>,
  )
}
function control(html: string) {
  return html.match(/<button[^>]*role="switch"[^>]*>/)?.[0] ?? ''
}
describe('refinement controls', () => {
  it('requires a selected service without a paid tier, with a working setup route', () => {
    const html = markup()
    expect(control(html)).toContain(' disabled=')
    expect(html).toContain('href="/settings/ai"')
    expect(html).not.toContain('Pro')
    expect(html).not.toContain('href="/settings/about"')
    data.settings.activeProviderId = 'fixture'
    expect(control(markup())).not.toContain(' disabled=')
  })
  it('keeps disabling available during recording while enabling waits for idle', () => {
    data.session.phase = 'polishing'
    expect(control(markup())).toContain(' disabled=')
    data.settings.polishEnabled = true
    expect(control(markup())).not.toContain(' disabled=')
    expect(control(markup())).toContain('aria-checked="true"')
  })
})
