import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

if (process.platform !== 'darwin' || process.arch !== 'arm64') {
  throw new Error('The candidate UI check currently requires Apple Silicon macOS.')
}
const root = fileURLToPath(new URL('../', import.meta.url))
const publicAudio = process.argv.slice(2).join(' ') === '--public-audio'
if (process.argv.length > 2 && !publicAudio) throw new Error('Unknown model check option')
for (const args of [
  ['scripts/diagnostics/prepare-model-check-catalog.mjs'],
  ['scripts/prepare-worker.mjs', '--release'],
  [
    'node_modules/@tauri-apps/cli/tauri.js',
    'build',
    '--features',
    publicAudio ? 'model-audio-check' : 'model-check',
    '--config',
    publicAudio
      ? 'src-tauri/tauri.model-audio-check.conf.json'
      : 'src-tauri/tauri.model-check.conf.json',
  ],
]) {
  const result = spawnSync(process.execPath, args, {
    cwd: root,
    stdio: 'inherit',
    env: {
      ...process.env,
      VITE_TARGET_AUDIT: '0',
      VITE_PUBLIC_AUDIO_CHECK: publicAudio ? '1' : '0',
      PATH: ['/usr/bin', '/bin', process.env.PATH ?? ''].join(path.delimiter),
    },
  })
  if (result.error) throw result.error
  if (result.status !== 0) process.exit(result.status ?? 1)
}
