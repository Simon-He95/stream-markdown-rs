import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'

export async function loadCandidate() {
  const entry = process.env.PARSER_CANDIDATE
  if (!entry)
    throw new Error('Set PARSER_CANDIDATE to the Rust/WASM adapter module path.')
  return import(pathToFileURL(resolve(entry)).href)
}
