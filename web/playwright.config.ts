import { defineConfig } from '@playwright/test'

// Smoke tests against the production build (`vite preview`).
export default defineConfig({
  testDir: 'tests',
  timeout: 60_000,
  reporter: [['list']],
  use: { baseURL: 'http://localhost:4173', viewport: { width: 1400, height: 860 } },
  webServer: {
    command: 'npm run build && npx vite preview --port 4173 --strictPort',
    url: 'http://localhost:4173',
    reuseExistingServer: !process.env.CI,
    timeout: 180_000,
  },
})
