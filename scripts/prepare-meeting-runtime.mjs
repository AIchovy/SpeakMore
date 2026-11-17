import { spawnSync } from 'node:child_process'
import { mkdirSync, copyFileSync, existsSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { verifySystemAudio } from './verify-system-audio.mjs'

const root = fileURLToPath(new URL('../', import.meta.url))
const workers = path.join(root, 'resources/workers')
const cache = path.join(root, '.cache/meeting-runtime')
const revision = 'c85b92c69c955961621193cd51da194f3cbcedf3'
function run(cmd, args) {
  const r = spawnSync(cmd, args, { cwd: root, stdio: 'inherit' })
  if (r.error || r.status !== 0) throw new Error(`${cmd} failed`)
}
mkdirSync(workers, { recursive: true })
mkdirSync(cache, { recursive: true })
run('/usr/bin/swiftc', [
  '-O',
  '-target',
  'arm64-apple-macos14.0',
  '-module-cache-path',
  path.join(cache, 'swift-modules'),
  'resources/meeting-decode.swift',
  '-o',
  path.join(workers, 'speakmore-meeting-decode'),
])
const source = path.join(cache, `llama.cpp-${revision}`)
if (!existsSync(source)) {
  if (process.env.CARGO_NET_OFFLINE === 'true')
    throw new Error('Prepare the pinned meeting runtime once while online before an offline build.')
  run('curl', [
    '--fail',
    '-L',
    `https://codeload.github.com/ggml-org/llama.cpp/tar.gz/${revision}`,
    '-o',
    path.join(cache, 'llama.tar.gz'),
  ])
  run('tar', ['-xzf', path.join(cache, 'llama.tar.gz'), '-C', cache])
}
run('cmake', [
  '-S',
  source,
  '-B',
  path.join(cache, 'build'),
  '-DCMAKE_BUILD_TYPE=Release',
  '-DCMAKE_OSX_DEPLOYMENT_TARGET=14.0',
  '-DBUILD_SHARED_LIBS=OFF',
  '-DGGML_NATIVE=OFF',
  '-DLLAMA_CURL=OFF',
  '-DLLAMA_BUILD_TESTS=OFF',
  '-DLLAMA_BUILD_SERVER=OFF',
  '-DLLAMA_BUILD_TOOLS=ON',
])
run('cmake', ['--build', path.join(cache, 'build'), '--target', 'llama-completion', '-j', '4'])
copyFileSync(
  path.join(cache, 'build/bin/llama-completion'),
  path.join(workers, 'speakmore-text-worker'),
)
copyFileSync(path.join(source, 'LICENSE'), path.join(workers, 'llama-LICENSE.txt'))
// The fixed helper performs only local inference and is signed as part of the app.

run('/usr/bin/swiftc', [
  '-O',
  '-target',
  'arm64-apple-macos14.0',
  '-emit-library',
  '-module-name',
  'SpeakMoreSystemAudio',
  '-module-cache-path',
  path.join(cache, 'swift-modules'),
  'resources/system-audio.swift',
  '-o',
  path.join(workers, 'libspeakmore-audio.dylib'),
])
verifySystemAudio(path.join(workers, 'libspeakmore-audio.dylib'))
const sherpa = JSON.parse(
  (await import('node:fs')).readFileSync(
    path.join(root, 'resources/sensevoice-native.lock.json'),
    'utf8',
  ),
)
const libs = path.join(root, '.cache/sensevoice/native', sherpa.directory, 'lib')
run('clang++', [
  '-std=c++17',
  '-O2',
  '-mmacosx-version-min=14.0',
  '-dynamiclib',
  '-Ivendor/sherpa-onnx',
  'resources/streaming-asr.cpp',
  ...sherpa.libraries.map((l) => path.join(libs, l.filename)),
  '-framework',
  'Foundation',
  '-o',
  path.join(workers, 'libspeakmore-streaming.dylib'),
])
const staticLibs = [
  'src/libllama.a',
  'ggml/src/libggml.a',
  'ggml/src/ggml-metal/libggml-metal.a',
  'ggml/src/ggml-blas/libggml-blas.a',
  'ggml/src/libggml-cpu.a',
  'ggml/src/libggml-base.a',
]
run('clang++', [
  '-std=c++17',
  '-O2',
  '-mmacosx-version-min=14.0',
  `-I${source}/include`,
  `-I${source}/ggml/include`,
  `-I${source}/vendor/nlohmann`,
  'resources/translation-worker.cpp',
  ...staticLibs.map((f) => path.join(cache, 'build', f)),
  '-framework',
  'Accelerate',
  '-framework',
  'Metal',
  '-framework',
  'Foundation',
  '-framework',
  'MetalKit',
  '-o',
  path.join(workers, 'speakmore-translation-worker'),
])
run(process.execPath, ['scripts/prepare-moss-runtime.mjs'])
