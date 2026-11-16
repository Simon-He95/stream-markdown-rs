import { execFileSync } from 'node:child_process'
import { mkdirSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const destination = fileURLToPath(new URL('../.reference/', import.meta.url))
const archive = fileURLToPath(new URL('../fixtures/oracle-source.tar.gz', import.meta.url))
mkdirSync(destination, { recursive: true })
execFileSync('tar', ['-xzf', archive, '-C', destination], { stdio: 'inherit' })
