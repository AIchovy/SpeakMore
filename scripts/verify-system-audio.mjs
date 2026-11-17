import { spawnSync } from 'node:child_process'

// No permission prompts or capture: inspect the exact library that will be shipped.
export function verifySystemAudio(library) {
  const result = spawnSync('/usr/bin/nm', ['-gU', library], { encoding: 'utf8' })
  if (result.error || result.status !== 0) throw new Error('Cannot inspect system audio library')
  const symbols = new Set(result.stdout.split(/\s+/))
  for (const name of ['sm_audio_start', 'sm_audio_stop', 'sm_audio_apps', 'sm_audio_free']) {
    if (!symbols.has(`_${name}`))
      throw new Error(`System audio library is missing exported entry point: ${name}`)
  }
  console.log('Verified system audio library entry points')
}
