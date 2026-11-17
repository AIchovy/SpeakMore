// Operator-only utility. Never included in the desktop bundle or invoked at startup.
import { generateKeyPairSync } from 'node:crypto'
import { lstat, mkdir, open } from 'node:fs/promises'
import path from 'node:path'

const [keyId, privateOutput, trustOutput] = process.argv.slice(2)
if (
  process.argv.length !== 5 ||
  !/^[a-z0-9-]{1,64}$/.test(keyId ?? '') ||
  !privateOutput ||
  !trustOutput
) {
  throw new Error(
    'Usage: node scripts/license-keygen.mjs KEY_ID PRIVATE_PEM_OUTPUT PUBLIC_TRUST_OUTPUT',
  )
}
for (const output of [privateOutput, trustOutput]) {
  try {
    await lstat(output)
    throw new Error('Refusing to replace an existing output')
  } catch (error) {
    if (error.code !== 'ENOENT') throw error
  }
  await mkdir(path.dirname(output), { recursive: true, mode: 0o700 })
  const parent = await lstat(path.dirname(output))
  if (!parent.isDirectory() || parent.isSymbolicLink())
    throw new Error('Output parent must be a real directory')
}
const { privateKey, publicKey } = generateKeyPairSync('ed25519')
const pem = Buffer.from(privateKey.export({ format: 'pem', type: 'pkcs8' }))
const jwk = publicKey.export({ format: 'jwk' })
if (jwk.kty !== 'OKP' || jwk.crv !== 'Ed25519' || !jwk.x) throw new Error('Unexpected key type')
const trust = Buffer.from(
  JSON.stringify({ schema: 1, keys: [{ key_id: keyId, public_key: jwk.x }] }, null, 2) + '\n',
)
async function exclusiveWrite(output, bytes, mode) {
  const file = await open(output, 'wx', mode)
  try {
    await file.writeFile(bytes)
    await file.sync()
  } finally {
    await file.close()
  }
}
try {
  await exclusiveWrite(privateOutput, pem, 0o600)
  await exclusiveWrite(trustOutput, trust, 0o644)
} finally {
  pem.fill(0)
}
console.log(
  'Created an independent license signing key and public trust file. No license was issued.',
)
