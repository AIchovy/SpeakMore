import { spawnSync } from 'node:child_process'
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'

// Test the same ad-hoc Hardened Runtime policy as the local app, not an unsigned Python host.
// No model downloads, permission requests, microphone capture, or screen recording.
export function verifyNativeLibraries(bundle, entitlements) {
  const directory = mkdtempSync(path.join(os.tmpdir(), 'speakmore-native-check-'))
  function run(cmd, args) {
    const result = spawnSync(cmd, args, { encoding: 'utf8' })
    if (result.error || result.status !== 0)
      throw new Error(`Native component verification failed: ${result.stderr || result.error}`)
  }
  try {
    const source = path.join(directory, 'check.c')
    const executable = path.join(directory, 'check')
    const permissions = path.join(directory, 'entitlements.plist')
    writeFileSync(permissions, entitlements)
    writeFileSync(
      source,
      `#include <dlfcn.h>\n#include <stdio.h>\nint main(int argc,char**argv){if(argc<3)return 2;void*p=dlopen(argv[1],RTLD_NOW|RTLD_LOCAL);if(!p){fprintf(stderr,"%s\\n",dlerror());return 1;}for(int i=2;i<argc;i++){if(!dlsym(p,argv[i])){fprintf(stderr,"Missing entry point: %s\\n",argv[i]);return 1;}}return 0;}\n`,
    )
    run('/usr/bin/clang', [source, '-o', executable])
    run('/usr/bin/codesign', [
      '--force',
      '--sign',
      '-',
      '--options',
      'runtime',
      '--entitlements',
      permissions,
      executable,
    ])
    const workers = path.join(bundle, 'Contents/Resources/workers')
    run(executable, [
      path.join(workers, 'libspeakmore-streaming.dylib'),
      'sm_asr_create',
      'sm_asr_fork',
      'sm_asr_accept',
      'sm_asr_destroy',
    ])
    run(executable, [
      path.join(workers, 'libspeakmore-audio.dylib'),
      'sm_audio_start',
      'sm_audio_stop',
      'sm_audio_apps',
      'sm_audio_free',
    ])
    console.log('Verified native libraries under the signed local app runtime policy')
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
}
