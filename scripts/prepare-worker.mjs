import { spawnSync } from 'node:child_process'
import { copyFileSync, mkdirSync, chmodSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const root = fileURLToPath(new URL('../', import.meta.url))
mkdirSync(path.join(root, 'resources/workers'), { recursive: true })
copyFileSync(
  path.join(root, 'resources/asr-models-NOTICE.txt'),
  path.join(root, 'resources/workers/ASR-models-NOTICE.txt'),
)
const release = process.argv.includes('--release')
const args = ['build', '-p', 'speakmore-asr-worker', '--bin', 'speakmore-asr-worker']
if (process.platform === 'darwin') args.push('--features', 'metal')
if (release) args.push('--release')
const result = spawnSync('cargo', args, {
  cwd: root,
  stdio: 'inherit',
  env: { ...process.env, CARGO_TARGET_DIR: path.join(root, 'target') },
})
if (result.status !== 0) process.exit(result.status ?? 1)
if (release) {
  const filename =
    process.platform === 'win32' ? 'speakmore-asr-worker.exe' : 'speakmore-asr-worker'
  const folder = path.join(root, 'resources/workers')
  mkdirSync(folder, { recursive: true })
  // Fixed bundled executable; model catalogs and WebViews cannot override this path.
  copyFileSync(path.join(root, 'target/release', filename), path.join(folder, filename))
  if (process.platform !== 'win32') chmodSync(path.join(folder, filename), 0o755)
}

if (process.platform === 'darwin' && process.arch === 'arm64') {
  const prepare = spawnSync(
    'python3',
    [
      'scripts/prepare-sensevoice-native.py',
      ...(process.env.CARGO_NET_OFFLINE === 'true' ? ['--offline'] : []),
    ],
    { cwd: root, stdio: 'inherit' },
  )
  if (prepare.status !== 0) process.exit(prepare.status ?? 1)
  // A separate Cargo invocation prevents Whisper/Metal feature unification into
  // the SenseVoice binary, and keeps ONNX out of the Whisper executable.
  const build = spawnSync(
    'cargo',
    [
      'build',
      '-p',
      'speakmore-asr-worker',
      '--no-default-features',
      '--features',
      'sensevoice',
      '--bin',
      'speakmore-sensevoice-worker',
      ...(release ? ['--release'] : []),
    ],
    {
      cwd: root,
      stdio: 'inherit',
      env: { ...process.env, CARGO_TARGET_DIR: path.join(root, 'target') },
    },
  )
  if (build.status !== 0) process.exit(build.status ?? 1)
  if (release) {
    const filename = 'speakmore-sensevoice-worker'
    const folder = path.join(root, 'resources/workers')
    copyFileSync(path.join(root, 'target/release', filename), path.join(folder, filename))
    chmodSync(path.join(folder, filename), 0o755)
  }
}

if (process.platform === 'darwin') {
  const result = spawnSync(process.execPath, ['scripts/prepare-meeting-runtime.mjs'], {
    cwd: root,
    stdio: 'inherit',
    env: process.env,
  })
  if (result.status !== 0) process.exit(result.status ?? 1)
}
