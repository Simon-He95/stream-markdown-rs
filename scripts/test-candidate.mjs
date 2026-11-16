import { spawnSync } from 'node:child_process'
import { loadCandidate } from './load-candidate.mjs'

const candidate = await loadCandidate()
if (typeof candidate.createSession !== 'function')
  throw new TypeError('The candidate adapter must export createSession.')

const behavior = spawnSync('pnpm', ['exec', 'vitest', 'run', 'test/behavior'], {
  stdio: 'inherit',
  env: { ...process.env, TEST_PARSER_CANDIDATE: '1' },
})
const differential = spawnSync('pnpm', ['exec', 'tsx', 'scripts/differential.mjs'], { stdio: 'inherit' })
if (behavior.error)
  throw behavior.error
if (differential.error)
  throw differential.error
process.exitCode = (behavior.status ?? 1) || (differential.status ?? 1)
