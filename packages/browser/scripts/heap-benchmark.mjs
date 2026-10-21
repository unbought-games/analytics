// CDP heap evidence only. This does not change the SDK runtime or assert a heap-byte budget.
import {chromium} from '@playwright/test';
import {writeFile, mkdir} from 'node:fs/promises';
import os from 'node:os';
import assert from 'node:assert/strict';
import {fixtureServer, propertyId} from '../tests/server.mjs';
const server = fixtureServer(); await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const endpoint = `http://127.0.0.1:${server.address().port}`;
const browser = await chromium.launch();
const options = {endpoint, propertyId, publicToken: 'synthetic-public-token', policyVersion: 1, profile: 'identified', choice: {enabled: true, persistentAttribution: true}, routes: ['/fixture', '/next'], queue: {flushIntervalMs: 60000}};
const runs = [];
async function measure(workload, enabled, repetition) {
  await fetch(`${endpoint}/test/reset`, {method: 'POST'});
  await fetch(`${endpoint}/test/config`, {method: 'POST', body: JSON.stringify({status: workload === 'outage' ? 503 : 0})});
  const context = await browser.newContext(); const page = await context.newPage();
  await page.goto(endpoint + '/fixture?utm_campaign=heap-fixture');
  if (enabled) await page.evaluate(async options => { window.client = window.Analytics.init(options); await window.client.ready; }, options);
  const cdp = await context.newCDPSession(page);
  await cdp.send('HeapProfiler.collectGarbage');
  const baseline = await cdp.send('Runtime.getHeapUsage');
  const samples = [baseline.usedSize];
  const sample = async () => { const heap = await cdp.send('Runtime.getHeapUsage'); samples.push(heap.usedSize); };
  if (workload === 'normal') {
    for (let segment = 0; segment < 5; segment++) {
      await page.evaluate(segment => {
        history.pushState({}, '', segment % 2 ? '/fixture' : '/next');
        for (let i = 0; i < 10; i++) document.querySelector('#button').click();
      }, segment);
      await sample();
      if (enabled) await page.evaluate(() => window.client.flush());
      await sample();
    }
  } else {
    // Yield every 100 clicks to inspect the page heap. No explicit GC between chunks.
    for (let chunk = 0; chunk < 100; chunk++) {
      await page.evaluate(() => { for (let i = 0; i < 100; i++) document.querySelector('#button').click(); });
      await sample();
    }
  }
  const finalBeforeGC = await cdp.send('Runtime.getHeapUsage');
  await cdp.send('HeapProfiler.collectGarbage');
  const retained = await cdp.send('Runtime.getHeapUsage');
  const coverage = enabled ? await page.evaluate(() => window.client.getCoverage()) : null;
  if (enabled) {
    assert.ok(coverage.highWaterEvents <= 200 && coverage.highWaterBytes <= 131072);
    if (workload === 'normal') { assert.equal(coverage.accepted, 56); assert.equal(coverage.dropped.queue, 0); }
    else { assert.ok(coverage.dropped.queue > 0); assert.equal(coverage.inFlightEvents, 0); }
    for (let i = 0; i < 40 && (await page.evaluate(() => window.client.getCoverage().queuedEvents)); i++) await page.evaluate(() => window.client.flush());
  }
  await cdp.send('HeapProfiler.collectGarbage');
  const released = await cdp.send('Runtime.getHeapUsage');
  const result = {workload, enabled, repetition, baselineBytes: baseline.usedSize, sampledPeakBytes: Math.max(...samples, finalBeforeGC.usedSize), sampledPeakDeltaBytes: Math.max(...samples, finalBeforeGC.usedSize) - baseline.usedSize, finalBeforeGCBytes: finalBeforeGC.usedSize, retainedAfterGCBytes: retained.usedSize, retainedDeltaBytes: retained.usedSize - baseline.usedSize, afterDrainAndGCBytes: released.usedSize, afterDrainDeltaBytes: released.usedSize - baseline.usedSize, sampleCount: samples.length + 1, samplesUsedBytes: samples, coverage};
  await context.close(); return result;
}
const range = values => ({min: Math.min(...values), median: [...values].sort((a,b) => a-b)[Math.floor(values.length/2)], max: Math.max(...values)});
try {
  // Interleave enabled and uninitialized controls; fresh contexts prevent identity/cache carryover.
  for (let repetition = 0; repetition < 5; repetition++) for (const workload of ['normal', 'outage']) for (const enabled of [false, true]) runs.push(await measure(workload, enabled, repetition));
  const summaries = [];
  for (const workload of ['normal', 'outage']) for (const enabled of [false, true]) {
    const group = runs.filter(run => run.workload === workload && run.enabled === enabled);
    summaries.push({workload, enabled, repetitions: group.length, sampledPeakBytes: range(group.map(run => run.sampledPeakBytes)), sampledPeakDeltaBytes: range(group.map(run => run.sampledPeakDeltaBytes)), retainedDeltaBytes: range(group.map(run => run.retainedDeltaBytes)), afterDrainDeltaBytes: range(group.map(run => run.afterDrainDeltaBytes))});
  }
  const result = {
    formatVersion: 1, measuredAt: new Date().toISOString(), implementationRevision: '9c377bd',
    platform: {os: os.platform(), arch: os.arch(), cpu: os.cpus()[0]?.model, logicalCPUs: os.cpus().length, node: process.version, chromium: browser.version()},
    method: {api: 'CDP Runtime.getHeapUsage usedSize and HeapProfiler.collectGarbage', scope: 'Whole page V8 JavaScript heap, not isolated SDK or process RSS; DOM/native/network memory excluded', baseline: 'After SDK readiness (one initial page event queued) or same bundle loaded without init; forced GC', normal: 'Five navigation transitions and 50 clicks; sample before/after each segment flush', outage: '10,000 clicks in 100-click chunks; sample after each chunk; 503 endpoint; no explicit GC between chunks; automatic GC may run', retained: 'Forced GC after workload, before outage queue drain; normal queue already drained', released: 'Exhaust bounded retries, then forced GC', controls: 'Same fixture and loaded bundle without init, five fresh-context runs per workload; interleaved with enabled runs', limitations: 'Sampled maxima are lower bounds on transient peaks. CDP polling changes scheduling and allocates inspector data. Automatic GC, JIT and browser bookkeeping create noise; retained deltas can be negative. No per-event heap-size or portable maximum-heap guarantee is inferred.'},
    summaries, runs
  };
  await mkdir(new URL('../benchmarks/', import.meta.url), {recursive: true});
  await writeFile(new URL('../benchmarks/heap.json', import.meta.url), JSON.stringify(result, null, 2) + '\n');
  console.log(JSON.stringify({platform: result.platform, summaries}, null, 2));
} finally { await browser.close(); await new Promise(resolve => server.close(resolve)); }
