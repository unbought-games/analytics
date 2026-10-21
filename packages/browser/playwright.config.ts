import {defineConfig} from '@playwright/test';
export default defineConfig({
  testDir: './tests', timeout: 20000, workers: 1, fullyParallel: false,
  use: {baseURL: 'http://127.0.0.1:41739', headless: true},
  webServer: {command: 'node tests/server.mjs', url: 'http://127.0.0.1:41739/fixture', reuseExistingServer: false},
  reporter: 'list'
});
