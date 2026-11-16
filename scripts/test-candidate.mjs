import { execFileSync } from 'node:child_process'
import { loadCandidate } from './load-candidate.mjs'

const candidate = await loadCandidate()
if (typeof candidate.createSession !== 'function')
  throw new TypeError('The candidate adapter must export createSession.')

execFileSync('pnpm', ['exec', 'vitest', 'run', 'test/behavior'], {
  stdio: 'inherit',
  env: { ...process.env, TEST_PARSER_CANDIDATE: '1' },
})
execFileSync('pnpm', ['exec', 'tsx', 'scripts/differential.mjs'], { stdio: 'inherit' })
