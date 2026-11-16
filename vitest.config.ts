import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    include: ['test/{original,behavior}/*.test.ts'],
    environment: 'node',
    restoreMocks: true,
    testTimeout: 10000,
  },
})
