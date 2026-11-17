import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import path from 'node:path'
import { existsSync } from 'node:fs'
import { verifyNativeLibraries } from './verify-native-libraries.mjs'
import { verifySystemAudio } from './verify-system-audio.mjs'

const root = fileURLToPath(new URL('../', import.meta.url))
if (process.platform !== 'darwin') {
  throw new Error(
    'This bundle command currently targets macOS. Windows packaging is not enabled yet.',
  )
}
function run(script, args, env = process.env) {
  const result = spawnSync(process.execPath, [path.join(root, script), ...args], {
    cwd: root,
    stdio: 'inherit',
    env,
  })
  if (result.error) throw result.error
  if (result.status !== 0) process.exit(result.status ?? 1)
}
run('scripts/prepare-worker.mjs', ['--release'])
// Resolve Apple's signing utilities ahead of similarly named Python shims.
// This only affects the child build process, never the user's shell configuration.
run(
  'node_modules/@tauri-apps/cli/tauri.js',
  ['build', '--config', 'src-tauri/tauri.release.conf.json'],
  {
    ...process.env,
    VITE_TARGET_AUDIT: '0',
    VITE_PUBLIC_AUDIO_CHECK: '0',
    PATH: ['/usr/bin', '/bin', process.env.PATH ?? ''].join(path.delimiter),
  },
)

// Check the signed artifact, not just its source plist: without this entitlement
// Hardened Runtime can reject microphone access before a consent prompt appears.
const bundle = path.join(root, 'target/release/bundle/macos/SpeakMore.app')
verifySystemAudio(path.join(bundle, 'Contents/Resources/workers/libspeakmore-audio.dylib'))
if (existsSync(path.join(bundle, 'Contents/Resources/workers/moss-python'))) {
  throw new Error('MOSS Python must be installed on demand, never included in the app bundle.')
}
const signed = spawnSync('/usr/bin/codesign', ['-d', '--entitlements', '-', '--xml', bundle], {
  encoding: 'utf8',
})
if (signed.status !== 0) throw new Error('Unable to read signed app entitlements.')
const decoded = spawnSync('/usr/bin/plutil', ['-convert', 'json', '-o', '-', '-'], {
  input: signed.stdout,
  encoding: 'utf8',
})
if (
  decoded.status !== 0 ||
  JSON.parse(decoded.stdout)['com.apple.security.device.audio-input'] !== true
) {
  throw new Error('Signed SpeakMore.app is missing the microphone audio-input entitlement.')
}
console.log('Verified microphone entitlement in the signed SpeakMore.app')

verifyNativeLibraries(bundle, signed.stdout)
