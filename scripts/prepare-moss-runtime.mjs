import { cpSync, copyFileSync, rmSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const root = fileURLToPath(new URL('../', import.meta.url))
const workers = path.join(root, 'resources/workers')
// Remove the legacy generated copy as well, so incremental builds stay small.
rmSync(path.join(workers, 'moss-python'), { recursive: true, force: true })
rmSync(path.join(workers, 'moss-package'), { recursive: true, force: true })
cpSync(path.join(root, 'resources/moss-package'), path.join(workers, 'moss-package'), {
  recursive: true,
  filter: (p) => !p.includes('__pycache__') && !p.endsWith('.pyc'),
})
copyFileSync(path.join(root, 'resources/moss-worker.py'), path.join(workers, 'moss-worker.py'))

copyFileSync(path.join(root, 'resources/speaker_identity.py'), path.join(workers, 'speaker_identity.py'))
