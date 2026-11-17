// Public development signer only. No production key file is read or created.
import { createPrivateKey, createPublicKey, sign } from 'node:crypto'
import { readFileSync, mkdirSync, writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const root = fileURLToPath(new URL('../../', import.meta.url))
const payload = JSON.parse(readFileSync(path.join(root, 'resources/model-catalog.payload.json')))
const original = payload.models.find((m) => m.model_id === 'sensevoice-small-int8' && !m.version)
if (!original) throw new Error('Expected the published SenseVoice baseline')
const candidate = structuredClone(original)
candidate.version = 'tiled256-v1'
candidate.engine_profile = 'sensevoice-tiled256-v1'
const graph = candidate.artifacts.find((a) => a.role === 'model')
graph.filename = 'model.tiled.onnx'
graph.sha256 = '46346d31c9314b4873aa16882f20955774cda4048a849c80db2a27117569a3d8'
graph.size_bytes = 239537160
// Import-only test source, matching the actor/worker fixture identity. This is
// not a published graph download. Do not use this catalog in a normal app.
graph.url = `https://huggingface.co/speakmore/test/resolve/${graph.revision}/model.tiled.onnx`
payload.models.push(candidate)
const key = createPrivateKey({
  key: Buffer.concat([
    Buffer.from('302e020100300506032b657004220420', 'hex'),
    Buffer.alloc(32, 81),
  ]),
  format: 'der',
  type: 'pkcs8',
})
const raw = Buffer.from(JSON.stringify(payload))
const keyId = 'model-check-public-fixture-only'
const envelope = {
  key_id: keyId,
  payload: raw.toString('base64'),
  signature: sign(null, raw, key).toString('base64'),
}
const trust = {
  key_id: keyId,
  public_key: createPublicKey(key)
    .export({ format: 'der', type: 'spki' })
    .subarray(-32)
    .toString('base64'),
}
const directory = path.join(root, 'src-tauri/fixtures/model-check')
mkdirSync(directory, { recursive: true })
for (const [name, data] of [
  ['catalog.signed.json', envelope],
  ['trust.json', trust],
]) {
  writeFileSync(path.join(directory, name), `${JSON.stringify(data, null, 2)}\n`)
}
console.log('Prepared public test catalog for the isolated Model Check build only.')
