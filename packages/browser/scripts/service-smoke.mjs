// Actual Chromium -> Rust ingestion -> disposable PostgreSQL contract regression.
import {spawn, execFileSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import {chromium} from '@playwright/test';
import assert from 'node:assert/strict';
import {fixtureServer} from '../tests/server.mjs';
const root = fileURLToPath(new URL('../../../', import.meta.url));
const name = `analytics-browser-smoke-${process.pid}`;
const docker = (...args) => execFileSync('docker', args, {encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe']}).trim();
let ownedDatabase = false; let service; let browser; let server;
try {
  let databaseURL = process.env.INGEST_TEST_DATABASE_URL;
  if (!databaseURL) {
    docker('run', '--rm', '-d', '--name', name, '-e', 'POSTGRES_HOST_AUTH_METHOD=trust', '-p', '127.0.0.1::5432', 'postgres:17'); ownedDatabase = true;
    const port = docker('port', name, '5432').split(':').at(-1);
    databaseURL = `postgres://postgres@127.0.0.1:${port}/postgres`;
    for (let attempt = 0; ; attempt++) {
      try { docker('exec', name, 'pg_isready', '-U', 'postgres'); break; }
      catch { if (attempt === 50) throw new Error('Disposable database did not become ready'); await new Promise(resolve => setTimeout(resolve, 100)); }
    }
  }
  server = fixtureServer(); await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  service = spawn('cargo', ['run', '--quiet', '-p', 'analytics-ingest', '--example', 'browser_harness'], {
    cwd: root, env: {...process.env, INGEST_TEST_DATABASE_URL: databaseURL, BROWSER_TEST_ORIGIN: origin, BROWSER_SCHEMA_FILE: fileURLToPath(new URL('../../../examples/static-site/schemas.json', import.meta.url))}, stdio: ['ignore', 'pipe', 'pipe']
  });
  const configured = await new Promise((resolve, reject) => {
    let output = ''; let errors = '';
    const timeout = setTimeout(() => reject(new Error('Rust browser harness startup timed out')), 180000);
    service.stdout.on('data', chunk => {
      output += chunk;
      const line = output.split('\n').find(line => line.startsWith('{'));
      if (line) { try { resolve(JSON.parse(line)); clearTimeout(timeout); } catch {} }
    });
    service.stderr.on('data', chunk => { errors = (errors + chunk).slice(-3000); });
    service.on('exit', code => { clearTimeout(timeout); if (code !== 0) reject(new Error(`Rust browser harness exited (${code}): ${errors}`)); });
    service.on('error', reject);
  });
  browser = await chromium.launch(); const page = await browser.newPage();
  const batches = []; const acknowledgements = [];
  page.on('request', request => { if (request.url().endsWith('/v1/ingest/browser')) batches.push(request.postDataJSON()); });
  page.on('response', response => { if (response.url().endsWith('/v1/ingest/browser')) acknowledgements.push(response.json()); });
  await page.goto(origin + '/fixture?utm_campaign=contract-regression&utm_source=synthetic');
  await page.evaluate(async config => {
    window.client = window.Analytics.init({endpoint: config.endpoint, propertyId: config.property_id, publicToken: config.token, policyVersion: 1, profile: 'identified', choice: {enabled: true, persistentAttribution: true}, routes: ['/fixture', '/next'], queue: {flushIntervalMs: 60000}});
    await window.client.ready;
    document.querySelector('#button').click(); history.pushState({}, '', '/next');
    await window.client.flush();
  }, configured);
  const firstStats = await page.evaluate(() => window.client.getCoverage());
  assert.equal(firstStats.accepted, 3, 'Tagged landing, interaction and SPA view accepted in one ordered batch');
  assert.equal(firstStats.rejected, 0);
  assert.equal(batches.length, 1); assert.equal(batches[0].events[0].touch_id, undefined);
  assert.equal(batches[0].events[1].touch_id, batches[0].events[0].event_id);
  const rows = await (await fetch(configured.endpoint + '/__test/events')).json();
  assert.equal(rows.length, 3); assert.equal(rows[0].campaign.utm_campaign, 'contract-regression');
  assert.equal(rows[1].campaign, null); assert.equal(rows[1].subject, null);
  const replay = await page.evaluate(async ({config, batch}) => {
    return (await fetch(config.endpoint + '/v1/ingest/browser', {method: 'POST', headers: {'Content-Type': 'application/json', 'X-Property-Token': config.token}, body: JSON.stringify(batch)})).json();
  }, {config: configured, batch: batches[0]});
  assert.deepEqual(replay.results.map(result => result.status), ['duplicate', 'duplicate', 'duplicate']);
  const forged = structuredClone(batches[0].events[0]); forged.event_id = crypto.randomUUID(); forged.subject = {namespace: 'account', id: 'forged'};
  const forgery = await page.evaluate(async ({config, event}) => {
    return (await fetch(config.endpoint + '/v1/ingest/browser', {method: 'POST', headers: {'Content-Type': 'application/json', 'X-Property-Token': config.token}, body: JSON.stringify({events: [event]})})).json();
  }, {config: configured, event: forged});
  assert.equal(forgery.results[0].status, 'rejected');
  const prior = await page.evaluate(() => window.client.getJourneyContext());
  await page.evaluate(async () => { window.client.reset(); document.querySelector('#button').click(); await window.client.flush(); });
  const after = await page.evaluate(() => window.client.getJourneyContext());
  assert.notEqual(after.visitor_id, prior.visitor_id); assert.equal(after.touch_id, null);
  const finalRows = await (await fetch(configured.endpoint + '/__test/events')).json();
  assert.equal(finalRows.length, 4); assert.equal(finalRows.at(-1).touch_id, null);
  for (const ack of await Promise.all(acknowledgements)) assert.ok(ack.results.every(result => ['accepted', 'duplicate', 'rejected'].includes(result.status)));
  console.log(JSON.stringify({result: 'passed', service: 'analytics-ingest real PostgreSQL and Chromium', acceptedRows: finalRows.length, sameBatchCampaignReference: true, exactReplayDeduplicated: true, forgedBrowserSubjectRejected: true, resetRotatesJourney: true}, null, 2));
} finally {
  await browser?.close();
  if (service && service.exitCode === null) { service.kill('SIGTERM'); await new Promise(resolve => service.once('exit', resolve)); }
  if (server) await new Promise(resolve => server.close(resolve));
  if (ownedDatabase) docker('rm', '-f', name);
}
