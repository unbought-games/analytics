import {chromium} from '@playwright/test';
import {readFile, writeFile, mkdir} from 'node:fs/promises';
import {gzipSync, brotliCompressSync} from 'node:zlib';
import os from 'node:os';
import assert from 'node:assert/strict';
import {fixtureServer, propertyId} from '../tests/server.mjs';
const server = fixtureServer(); await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const endpoint = `http://127.0.0.1:${server.address().port}`;
const browser = await chromium.launch();
const page = await browser.newPage();
const options = {endpoint, propertyId, publicToken: 'synthetic-public-token', policyVersion: 1, profile: 'identified', choice: {enabled: true, persistentAttribution: true}, routes: ['/fixture', '/next'], queue: {flushIntervalMs: 60000}};
const config = async body => { await fetch(`${endpoint}/test/config`, {method: 'POST', body: JSON.stringify(body)}); };
const sorted = values => [...values].sort((a,b) => a-b);
const percentile = (values, p) => sorted(values)[Math.min(values.length-1, Math.ceil(values.length*p)-1)];
try {
  await page.goto(endpoint + '/fixture?utm_campaign=synthetic-benchmark');
  const init = [];
  for (let i = 0; i < 30; i++) {
    const timing = await page.evaluate(async options => {
      const start = performance.now(); const client = window.Analytics.init(options); const sync = performance.now() - start;
      await client.ready; const ready = performance.now() - start; await client.shutdown(); return {sync, ready};
    }, options);
    if (i >= 5) init.push(timing);
  }
  await page.evaluate(async options => { window.client = window.Analytics.init(options); await window.client.ready; }, options);
  const interaction = []; let normal;
  for (let segment = 0; segment < 5; segment++) {
    const durations = await page.evaluate(async segment => {
      history.pushState({}, '', segment % 2 ? '/fixture' : '/next');
      const button = document.querySelector('#button'); const times = [];
      for (let i = 0; i < 10; i++) { const t = performance.now(); button.click(); times.push(performance.now() - t); }
      await window.client.flush(); return times;
    }, segment);
    interaction.push(...durations);
  }
  normal = await page.evaluate(() => window.client.getCoverage());
  await page.evaluate(() => window.client.shutdown());
  await config({status: 503});
  await page.evaluate(async options => { window.client = window.Analytics.init(options); await window.client.ready; }, options);
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('HeapProfiler.collectGarbage');
  const before = await cdp.send('Runtime.getHeapUsage');
  const burst = await page.evaluate(() => {
    const button = document.querySelector('#button'); const start = performance.now();
    for (let i = 0; i < 10000; i++) button.click();
    return {milliseconds: performance.now() - start, coverage: window.client.getCoverage()};
  });
  const peak = await cdp.send('Runtime.getHeapUsage');
  await cdp.send('HeapProfiler.collectGarbage');
  const retained = await cdp.send('Runtime.getHeapUsage');
  for (let i = 0; i < 40; i++) await page.evaluate(() => window.client.flush());
  const outage = await page.evaluate(() => window.client.getCoverage());
  const bundles = {};
  for (const name of ['index.js', 'index.cjs', 'analytics.global.js']) {
    const bytes = await readFile(new URL('../dist/' + name, import.meta.url));
    bundles[name] = {minifiedBytes: bytes.length, gzipBytes: gzipSync(bytes).length, brotliBytes: brotliCompressSync(bytes).length};
  }
  const result = {
    formatVersion: 1, measuredAt: new Date().toISOString(), platform: {os: os.platform(), arch: os.arch(), cpu: os.cpus()[0]?.model, logicalCPUs: os.cpus().length, node: process.version, chromium: browser.version()},
    workload: {warmupInitializations: 5, measuredInitializations: 25, normalNavigationTransitions: 5, normalClicks: 50, burstClicks: 10000, profile: 'identified', policyFixture: 'local controlled HTTP, not Rust service', flush: 'explicit at each normal segment; failed endpoint during burst; 40 manual bounded drain attempts'},
    budgets: {esmGzipBytes: 12288, synchronousInitializationP95Ms: 5, interactionP95Ms: 2, queueEvents: 200, queueEncodedBytes: 131072},
    bundles,
    initializationMs: {syncP50: percentile(init.map(t => t.sync), .5), syncP95: percentile(init.map(t => t.sync), .95), readyP50: percentile(init.map(t => t.ready), .5), readyP95: percentile(init.map(t => t.ready), .95)},
    interactionMs: {p50: percentile(interaction, .5), p95: percentile(interaction, .95), burstTotal: burst.milliseconds, burstAmortized: burst.milliseconds/10000},
    normal, burst: burst.coverage, outage,
    heapBytes: {before: before.usedSize, beforeGC: peak.usedSize, retainedAfterGC: retained.usedSize, retainedDelta: retained.usedSize-before.usedSize, note: 'Chromium total page JS heap; allocator/GC measurement, not a portable exact SDK heap bound'}
  };
  assert.ok(bundles['index.js'].gzipBytes <= result.budgets.esmGzipBytes, 'Compressed bundle budget');
  assert.ok(result.initializationMs.syncP95 <= result.budgets.synchronousInitializationP95Ms, 'Initialization budget');
  assert.ok(result.interactionMs.p95 <= result.budgets.interactionP95Ms, 'Interaction budget');
  assert.ok(burst.coverage.highWaterEvents <= 200 && burst.coverage.highWaterBytes <= 131072, 'Queue bounds');
  assert.equal(normal.dropped.queue, 0); assert.equal(normal.accepted, 56);
  assert.equal(outage.queuedEvents, 0); assert.equal(outage.inFlightEvents, 0);
  await mkdir(new URL('../benchmarks/', import.meta.url), {recursive: true});
  await writeFile(new URL('../benchmarks/latest.json', import.meta.url), JSON.stringify(result, null, 2) + '\n');
  console.log(JSON.stringify(result, null, 2));
} finally { await browser.close(); await new Promise(resolve => server.close(resolve)); }
