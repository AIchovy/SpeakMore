import { describe, expect, it } from 'vitest'
import { createPreview } from './preview'
import type { DictionaryEntry, Snapshot, ModelsSnapshot } from './types'

describe('browser workbench isolation', () => {
  it('never saves provider keys or pretends to test native AI connections', async () => {
    const preview = createPreview()
    for (const command of [
      'enable_polishing',
      'save_provider',
      'test_provider',
      'cancel_provider_test',
      'select_provider',
      'delete_provider',
      'retry_provider_cleanup',
    ]) {
      await expect(preview(command)).rejects.toMatchObject({ code: 'DesktopRequired' })
    }
    expect(await preview('get_providers')).toMatchObject({
      rows: [],
      activeProviderId: null,
      busy: false,
      cleanupPending: 0,
    })
  })
  it('never pretends to record or transcribe, even after settings change', async () => {
    const preview = createPreview()
    await preview('update_settings', { expectedRevision: 1, patch: { historyEnabled: true } })
    await expect(preview('start_session')).rejects.toMatchObject({ code: 'DesktopRequired' })
    await expect(preview('microphone_permission_action', { request: true })).rejects.toMatchObject({
      code: 'DesktopRequired',
    })
    await expect(preview('install_model')).rejects.toMatchObject({ code: 'DesktopRequired' })
    for (const command of [
      'recording_control',
      'retry_recording_shortcut',
      'preview_recording_panel',
      'begin_shortcut_edit',
      'shortcut_edit_heartbeat',
      'save_recording_shortcut',
      'export_history',
    ]) {
      await expect(preview(command)).rejects.toMatchObject({ code: 'DesktopRequired' })
    }
    const snapshot = (await preview('get_snapshot')) as Snapshot
    expect(snapshot.canStart).toBe(false)
    expect(snapshot.recentResults).toEqual([])
    expect(snapshot.session.maxDurationMs).toBe(300_000)
  })
  it('exposes catalog metadata but never downloads, deletes or selects native models', async () => {
    const preview = createPreview()
    const catalog = (await preview('list_models')) as ModelsSnapshot
    expect(catalog.models).toHaveLength(5)
    const sensevoice = catalog.models.find((m) => m.family === 'sensevoice')!
    expect(sensevoice.dictionaryPrompt).toBe(false)
    expect(sensevoice.languages).toEqual(['zh', 'yue', 'en', 'ja', 'ko'])
    expect(sensevoice.sizeBytes).toBe(241862836)
    expect(sensevoice.artifacts.map((file) => file.role).sort()).toEqual(['model', 'tokens', 'vad'])
    expect(sensevoice.artifacts.reduce((sum, file) => sum + file.sizeBytes, 0)).toBe(
      sensevoice.sizeBytes,
    )
    const id = catalog.models[0].modelId
    for (const command of [
      'download_model',
      'import_model',
      'cancel_model_download',
      'delete_model',
      'select_model',
    ]) {
      await expect(preview(command, { modelId: id })).rejects.toMatchObject({
        code: 'DesktopRequired',
      })
    }
    catalog.models[0].state = 'installed'
    expect(
      ((await preview('list_models')) as ModelsSnapshot).models.every((m) => m.state === 'absent'),
    ).toBe(true)
    expect(((await preview('get_snapshot')) as Snapshot).settings.activeModelId).toBeNull()
  })
  it('isolates state across reloads and protects snapshots from mutation', async () => {
    const first = createPreview(),
      second = createPreview()
    await first('upsert_dictionary', {
      expectedRevision: 1,
      input: { term: 'SpeakMore', locale: 'all', enabled: true, pinned: true },
    })
    const snapshot = (await first('get_snapshot')) as Snapshot
    snapshot.dictionary[0].term = 'tampered'
    expect(((await first('get_snapshot')) as Snapshot).dictionary[0].term).toBe('SpeakMore')
    expect(((await second('get_snapshot')) as Snapshot).dictionary).toEqual([])
    expect(await first('list_history')).toEqual({ items: [], nextCursor: null })
  })
  it('rejects conflicting edits and preserves the previous entry', async () => {
    const preview = createPreview()
    const entry = (await preview('upsert_dictionary', {
      expectedRevision: 1,
      input: { term: 'Original', locale: 'all', enabled: true, pinned: false },
    })) as DictionaryEntry
    await expect(
      preview('upsert_dictionary', { expectedRevision: 1, input: { ...entry, term: 'Overwrite' } }),
    ).rejects.toMatchObject({ code: 'RevisionConflict' })
    expect(((await preview('get_snapshot')) as Snapshot).dictionary[0].term).toBe('Original')
  })
})
