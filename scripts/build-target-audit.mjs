import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

if (process.platform !== 'darwin') throw new Error('The native target audit runs on macOS only.')
const root = fileURLToPath(new URL('../', import.meta.url))
const result = spawnSync(
  process.execPath,
  [
    path.join(root, 'node_modules/@tauri-apps/cli/tauri.js'),
    'build',
    '--features',
    'target-audit',
    '--config',
    'src-tauri/tauri.audit.conf.json',
  ],
  {
    cwd: root,
    stdio: 'inherit',
    env: {
      ...process.env,
      VITE_TARGET_AUDIT: '1',
      VITE_PUBLIC_AUDIO_CHECK: '0',
      PATH: ['/usr/bin', '/bin', process.env.PATH ?? ''].join(path.delimiter),
    },
  },
)
if (result.error) throw result.error
process.exit(result.status ?? 1)
