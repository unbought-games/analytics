import {test, expect, type Page, type APIRequestContext} from '@playwright/test';
const propertyId = '11111111-1111-4111-8111-111111111111';
const audienceId = '22222222-2222-4222-8222-222222222222';
const defaults = {endpoint: 'http://127.0.0.1:41739', propertyId, publicToken: 'synthetic-public-token', profile: 'identified', policyVersion: 1, choice: {enabled: true, detailedProperties: true, persistentAttribution: true, crossProperty: true}, routes: ['/fixture', '/', '/next', '/a', '/b', '/private/:id'], handoffOrigins: {'https://destination.example': audienceId}, queue: {flushIntervalMs: 60000}, schemas: {experiment_result: {variant: {type: 'string', values: ['a','b']}, count: {type: 'integer'}, email: {type: 'string'}, password: {type: 'string'}}}};
async function init(page: Page, options = {}) {
  await page.evaluate(async options => { (window as any).client = (window as any).Analytics.init(options); await (window as any).client.ready; }, {...defaults, ...options});
}
async function flush(page: Page) { await page.evaluate(() => (window as any).client.flush()); }
async function state(request: APIRequestContext) { return (await request.get('/test/state')).json(); }
async function events(request: APIRequestContext) { return (await state(request)).requests.filter((r: any) => r.path === '/v1/ingest/browser').flatMap((r: any) => r.body.events); }
async function coverage(page: Page) { return page.evaluate(() => (window as any).client.getCoverage()); }
test.beforeEach(async ({page, request}) => { await request.post('/test/reset'); await page.goto('/fixture'); });

test('initialization is idempotent across bundle copies; SPA navigation appears once', async ({page, request}) => {
  await init(page);
  expect(await page.evaluate(options => (window as any).client === (window as any).Analytics.init(options), defaults)).toBe(true);
  await page.addScriptTag({url: '/dist/analytics.global.js'});
  expect(await page.evaluate(options => (window as any).client === (window as any).Analytics.init(options), defaults)).toBe(true);
  await page.evaluate(() => { history.pushState({}, '', '/a'); history.replaceState({}, '', '/a'); history.replaceState({}, '', '/b'); });
  await page.goBack();
  await flush(page);
  const views = (await events(request)).filter((e: any) => e.name === 'page_view');
  expect(views.map((e: any) => e.context.page)).toEqual(['/fixture', '/a', '/b', '/fixture'].map(path => defaults.endpoint + path));
  expect(new Set(views.map((e: any) => e.event_id)).size).toBe(4);
});

test('delegated semantic controls, open shadow roots, keyboard activation, forms and exclusions', async ({page, request}) => {
  await init(page);
  await page.locator('#button').click();
  await page.locator('#button').press('Enter');
  await page.locator('#link').click();
  await page.locator('#tab').click();
  await page.locator('#menu').click();
  await page.locator('#shadow button').click();
  await page.locator('#excluded').click();
  await page.locator('#editable').click();
  await page.locator('#submit').click();
  await page.locator('#required').fill('Never transmit this value');
  await page.locator('#submit').click();
  await page.locator('summary').click();
  await expect.poll(async () => (await coverage(page)).produced).toBe(13);
  await flush(page);
  const collected = await events(request);
  expect(collected.filter((e: any) => e.name === 'control_activate')).toHaveLength(9);
  expect(collected.filter((e: any) => e.name === 'form_validation')).toHaveLength(1);
  expect(collected.filter((e: any) => e.name === 'form_submit')).toHaveLength(1);
  expect(collected.some((e: any) => e.context.control === 'shadow_control')).toBe(true);
  expect(collected.some((e: any) => e.name === 'disclosure_change')).toBe(true);
  expect(JSON.stringify(collected)).not.toMatch(/Never transmit|synthetic@example|private text|hidden|#secret|password/);
});

test('URL, referrer, campaign and explicit property sanitization survives actual transmission', async ({page, request}) => {
  await page.goto('/fixture?utm_campaign=autumn%20launch&utm_medium=email&utm_source=newsletter&utm_term=bad%40example.invalid&utm_content=%3Cscript%3E&experiment=trial-1&password=super-secret#private');
  await init(page);
  await page.evaluate(() => {
    (window as any).client.track('experiment_result', {variant: 'a', count: 3, email: 'synthetic@example.invalid', password: 'never-send', extra: 'never-send'});
    history.pushState({}, '', '/private/synthetic@example.invalid?token=never-send#hidden');
  });
  await flush(page);
  const collected = await events(request);
  expect(collected[0].campaign).toEqual({utm_campaign: 'autumn launch', utm_medium: 'email', utm_source: 'newsletter', experiment_id: 'trial-1'});
  expect(collected[1].properties).toEqual({variant: 'a', count: 3});
  expect(collected[2].context.page).toBe(defaults.endpoint + '/private/:redacted');
  expect(JSON.stringify(collected)).not.toMatch(/super-secret|never-send|synthetic@|script|#private|\?token|password|"subject"/);
  expect((await state(request)).requests.every((r: any) => r.referrer === null)).toBe(true);
});

test('minimal captures breadth without IDs, durable storage, identity or handoff', async ({page, request}) => {
  await init(page, {profile: 'minimal'});
  await page.locator('#button').click(); await flush(page);
  const collected = await events(request);
  expect(collected).toHaveLength(2);
  for (const event of collected) { expect(event.visit_id).toBeUndefined(); expect(event.visitor_id).toBeUndefined(); expect(event.touch_id).toBeUndefined(); }
  expect(await page.evaluate(() => localStorage.length + sessionStorage.length)).toBe(0);
  await expect(page.evaluate(() => (window as any).client.identify('synthetic-valid-looking-proof'))).rejects.toThrow('Identified collection is disabled');
  await expect(page.evaluate(() => (window as any).client.createHandoff('https://destination.example'))).rejects.toThrow('disabled');
});

test('journey retains visit only within tab/session and cannot escalate server profile', async ({page, request}) => {
  await request.post('/test/config', {data: {profile: 'journey', crossProperty: false}});
  await init(page);
  const before = await page.evaluate(() => (window as any).client.getJourneyContext());
  await page.reload(); await init(page); await flush(page);
  const collected = await events(request);
  expect(collected[0].visit_id).toBe(before.visit_id);
  expect(collected[0].visitor_id).toBeUndefined();
  expect(collected[0].collection.profile).toBe('journey');
  await page.evaluate(() => (window as any).client.setCollectionChoice({enabled: true, profile: 'identified', crossProperty: true}));
  await expect(page.evaluate(() => (window as any).client.createHandoff('https://destination.example'))).rejects.toThrow('disabled');
  expect(await page.evaluate(() => localStorage.length)).toBe(0);
});

test('opt-out purges queue and IDs; re-enable is a new visit; pending validation is purged', async ({page, request}) => {
  await init(page);
  const before = await page.evaluate(() => (window as any).client.getJourneyContext());
  await page.evaluate(() => { (document.querySelector('#form') as HTMLFormElement).checkValidity(); (window as any).client.setCollectionChoice({enabled: false}); });
  await page.locator('#button').click(); await flush(page);
  expect(await events(request)).toEqual([]);
  expect(await page.evaluate(() => localStorage.length + sessionStorage.length)).toBe(0);
  await page.evaluate(() => (window as any).client.setCollectionChoice({enabled: true}));
  await flush(page);
  const after = (await events(request))[0];
  expect(after.visit_id).not.toBe(before.visit_id); expect(after.visitor_id).not.toBe(before.visitor_id);
  expect((await state(request)).requests.find((r: any) => r.path === '/v1/privacy/withdraw').body).toMatchObject({visit_id: before.visit_id, visitor_id: before.visitor_id});
});

test('GPC and DNT prevent collection even when visitor opts in', async ({page, request}) => {
  await page.addInitScript(() => Object.defineProperty(navigator, 'globalPrivacyControl', {value: true}));
  await page.reload(); await init(page); await page.locator('#button').click(); await flush(page);
  expect(await events(request)).toEqual([]);
  expect((await coverage(page)).preferenceBlocked).toBe(true);
});

test('retry reuses exact payload/ID, queue bound includes in-flight, and failures exhaust', async ({page, request}) => {
  await request.post('/test/config', {data: {status: 503, delay: 100}});
  await init(page, {queue: {maxEvents: 10, maxBytes: 10000, batchSize: 4, maxAttempts: 2, flushIntervalMs: 60000}});
  await page.evaluate(() => { for (let i = 0; i < 100; i++) (document.querySelector('#button') as HTMLButtonElement).click(); (window as any).flushing = (window as any).client.flush(); });
  await page.evaluate(() => { for (let i = 0; i < 100; i++) (document.querySelector('#button') as HTMLButtonElement).click(); });
  expect((await coverage(page)).highWaterEvents).toBeLessThanOrEqual(10);
  expect((await coverage(page)).highWaterBytes).toBeLessThanOrEqual(10000);
  await page.evaluate(() => (window as any).flushing);
  for (let i = 0; i < 6; i++) await flush(page);
  const requests = (await state(request)).requests.filter((r: any) => r.path === '/v1/ingest/browser');
  const byID = new Map<string, any[]>();
  for (const req of requests) for (const event of req.body.events) byID.set(event.event_id, [...byID.get(event.event_id) ?? [], event]);
  for (const attempts of byID.values()) { expect(attempts).toHaveLength(2); expect(attempts[0]).toEqual(attempts[1]); }
  const stats = await coverage(page);
  expect(stats.queuedEvents).toBe(0); expect(stats.dropped.exhausted).toBe(10); expect(stats.dropped.queue).toBe(191);
});

test('per-event permanent rejection and duplicate ACK do not retry', async ({page, request}) => {
  await request.post('/test/config', {data: {reject: true, duplicate: true}});
  await init(page); await page.locator('#button').click(); await flush(page); await flush(page);
  const stats = await coverage(page);
  expect(stats.accepted).toBe(1); expect(stats.rejected).toBe(1); expect(stats.requests).toBe(1);
});

test('campaign first touch survives reload and untagged browsing, later touch does not overwrite it', async ({page, request}) => {
  await page.goto('/fixture?utm_campaign=first'); await init(page); await flush(page);
  const first = (await events(request))[0];
  expect(first.touch_id).toBeUndefined();
  await page.evaluate(() => history.pushState({}, '', '/next')); await flush(page);
  await page.reload(); await init(page); await flush(page);
  let all = await events(request);
  expect(all.at(-1).touch_id).toBe(first.event_id); expect(all.at(-1).campaign).toBeUndefined();
  await page.evaluate(() => history.pushState({}, '', '/a?utm_campaign=later')); await flush(page);
  all = await events(request); const later = all.at(-1);
  expect(later.touch_id).toBeUndefined(); expect(later.event_id).not.toBe(first.event_id);
  expect(await page.evaluate(() => (window as any).client.getJourneyContext())).toMatchObject({touch_id: first.event_id});
});

test('expired attribution and idle journey visits are rotated using controlled browser clock', async ({page, request}) => {
  await page.clock.install();
  await page.goto('/fixture?utm_campaign=old'); await init(page, {profile: 'journey', sessionTimeoutMs: 60000, attributionWindowMs: 60000}); await flush(page);
  const before = (await events(request))[0];
  await page.clock.fastForward(61000);
  await page.evaluate(() => history.pushState({}, '', '/next')); await flush(page);
  const after = (await events(request)).at(-1);
  expect(after.touch_id).toBeUndefined(); expect(after.visit_id).not.toBe(before.visit_id);
});

test('identity proof is bound to journey; reset removes previous account identifiers and campaign', async ({page, request}) => {
  await page.goto('/fixture?utm_campaign=first'); await init(page);
  const before = await page.evaluate(() => (window as any).client.getJourneyContext());
  const proof = await (await request.post('/test/proof', {data: before})).json();
  await page.evaluate(proof => (window as any).client.identify(proof), proof.proof);
  await expect(page.evaluate(() => (window as any).client.identify('arbitrary-account-id'))).rejects.toThrow('Account switch requires');
  await page.locator('#button').click(); await flush(page);
  const after = (await events(request)).at(-1);
  expect(after.visitor_id).not.toBe(before.visitor_id); expect(after.touch_id).toBeUndefined(); expect(after.subject).toBeUndefined();
});

test('handoffs require allowlisted audience and opt-in; trusted redemption uses bridge only', async ({page, request}) => {
  await page.goto('/fixture?utm_campaign=handoff'); await init(page);
  await expect(page.evaluate(() => (window as any).client.createHandoff('https://evil.example'))).rejects.toThrow('allowlisted');
  const handoff = await page.evaluate(() => (window as any).client.createHandoff('https://destination.example/signup'));
  expect(handoff).toMatch(/^synthetic-handoff-/);
  const sent = (await state(request)).requests.find((r: any) => r.path === '/v1/journey/handoff');
  expect(sent.body.audience).toBe(audienceId); expect(sent.body.touch_id).toBe((await events(request))[0].event_id);
  await expect(page.evaluate(handoff => (window as any).client.redeemHandoff(handoff), handoff)).rejects.toThrow('redeemBridge');
});

test('blocked storage degrades to memory, shutdown removes listeners and permits clean init', async ({page, request}) => {
  await page.evaluate(() => { Storage.prototype.setItem = () => { throw new Error('blocked'); }; });
  await init(page); await page.locator('#button').click(); await page.evaluate(() => (window as any).client.shutdown());
  expect((await coverage(page)).storageUnavailable).toBe(true);
  const count = (await events(request)).length;
  await page.locator('#button').click(); await page.evaluate(() => history.pushState({}, '', '/a'));
  expect((await events(request)).length).toBe(count);
  await init(page); await page.locator('#button').click(); await flush(page);
  expect((await events(request)).length).toBe(count + 2);
});

test('policy fetch fails closed, startup explicit drops are measurable, no persistent retry', async ({page, request}) => {
  await request.post('/test/config', {data: {policyFailure: true}});
  await page.evaluate(options => { (window as any).client = (window as any).Analytics.init(options); (window as any).client.track('experiment_result', {count: 1}); }, defaults);
  await expect(page.evaluate(() => (window as any).client.ready)).rejects.toThrow('policy unavailable');
  await page.locator('#button').click();
  expect(await events(request)).toEqual([]); expect((await coverage(page)).dropped.notReady).toBe(1);
  expect((await state(request)).requests).toHaveLength(1);
});

test('initial tagged landing survives asynchronous policy loading and SPA transition', async ({page, request}) => {
  await request.post('/test/config', {data: {policyDelay: 100}});
  await page.goto('/fixture?utm_campaign=early');
  await page.evaluate(options => { (window as any).client = (window as any).Analytics.init(options); history.replaceState({}, '', '/next'); }, defaults);
  await page.evaluate(() => (window as any).client.ready); await flush(page);
  const collected = await events(request);
  expect(collected.map((e: any) => e.context.page)).toEqual([defaults.endpoint + '/fixture', defaults.endpoint + '/next']);
  expect(collected[0].campaign.utm_campaign).toBe('early');
  expect(collected[1].touch_id).toBe(collected[0].event_id);
});

test('network timeout and missing acknowledgements terminate retries within bounds', async ({page, request}) => {
  await request.post('/test/config', {data: {delay: 200}});
  await init(page, {queue: {requestTimeoutMs: 50, maxAttempts: 2, flushIntervalMs: 60000}});
  await flush(page); await flush(page);
  expect((await coverage(page)).dropped.exhausted).toBe(1);
  await request.post('/test/config', {data: {delay: 0, badAck: true}});
  await page.locator('#button').click(); await flush(page); await flush(page);
  expect((await coverage(page)).dropped.exhausted).toBe(2);
  expect((await coverage(page)).queuedEvents).toBe(0);
});

test('pagehide makes a bounded best-effort keepalive delivery and dialog capture has no contents', async ({page, request}) => {
  await init(page);
  await page.evaluate(() => (document.querySelector('#dialog') as HTMLDialogElement).showModal());
  await expect.poll(async () => (await coverage(page)).produced).toBe(2);
  await page.evaluate(() => (document.querySelector('#dialog') as HTMLDialogElement).close());
  await expect.poll(async () => (await coverage(page)).produced).toBeGreaterThanOrEqual(3);
  await page.goto('/next');
  await expect.poll(async () => (await events(request)).length).toBeGreaterThanOrEqual(3);
  expect(JSON.stringify(await events(request))).not.toContain('Dialog contents');
});

test('logout in another tab clears an identified shared-device boundary', async ({page, context, request}) => {
  await init(page); await flush(page);
  const before = await page.evaluate(() => (window as any).client.getJourneyContext());
  const other = await context.newPage(); await other.goto('/fixture'); await init(other); await flush(other);
  expect((await other.evaluate(() => (window as any).client.getJourneyContext())).visitor_id).toBe(before.visitor_id);
  await page.evaluate(() => (window as any).client.reset());
  await expect.poll(async () => (await other.evaluate(() => (window as any).client.getJourneyContext())).visitor_id).toBeNull();
  await other.locator('#button').click(); await flush(other);
  expect((await events(request)).at(-1).visitor_id).not.toBe(before.visitor_id);
});

test('malformed and duplicate campaign values are discarded; no arbitrary URL path collection', async ({page, request}) => {
  await page.goto('/private-name?utm_campaign=one&utm_campaign=two&utm_source=%EF%BF%BD&experiment=%3Cscript%3E');
  await init(page); await flush(page);
  const event = (await events(request))[0];
  expect(event.campaign).toBeUndefined(); expect(event.context.page).toBe(defaults.endpoint + '/[redacted]');
});

test('turning off detailed custom properties preserves broad interaction capture', async ({page, request}) => {
  await init(page, {choice: {enabled: true, detailedProperties: false}});
  await page.evaluate(() => (window as any).client.track('experiment_result', {variant: 'a', count: 42}));
  await page.locator('#button').click(); await flush(page);
  const collected = await events(request);
  expect(collected.find((e: any) => e.name === 'experiment_result').properties).toEqual({});
  expect(collected.some((e: any) => e.name === 'control_activate')).toBe(true);
});

test('opaque hexadecimal handoff redemption is forwarded only through a trusted application bridge', async ({page, request}) => {
  await page.evaluate(async options => {
    (window as any).client = (window as any).Analytics.init({...options, redeemBridge: async (request: unknown) => { (window as any).redeemed = request; }});
    await (window as any).client.ready;
    await (window as any).client.redeemHandoff('ab'.repeat(32));
  }, defaults);
  const redemption = await page.evaluate(() => (window as any).redeemed);
  expect(redemption).toMatchObject({handoff: 'ab'.repeat(32), audience: propertyId});
  expect(redemption.visit_id).toBeTruthy(); expect(redemption.visitor_id).toBeTruthy();
  expect((await state(request)).requests.some((r: any) => r.path === '/v1/journey/redeem')).toBe(false);
});

test('DNT prevents collection and explicit browser tracks cannot claim authoritative outcomes', async ({page, request}) => {
  await page.addInitScript(() => Object.defineProperty(navigator, 'doNotTrack', {value: '1'}));
  await page.reload(); await init(page);
  const accepted = await page.evaluate(() => (window as any).client.track('signup', {account: 'fake'}));
  expect(accepted).toBe(false); await flush(page); expect(await events(request)).toEqual([]);
});

test('queue overflow is reported as a delta once, not repeatedly on every later event', async ({page, request}) => {
  await init(page, {queue: {maxEvents: 1, flushIntervalMs: 60000}});
  await page.evaluate(() => { for (let i = 0; i < 5; i++) (document.querySelector('#button') as HTMLButtonElement).click(); });
  await flush(page); await page.locator('#button').click(); await flush(page); await page.locator('#button').click(); await flush(page);
  expect((await events(request)).map((e: any) => e.collection.dropped_events)).toEqual([0,5,0]);
});

test('identified attribution persists across visits only when separately enabled', async ({page, request}) => {
  await page.clock.install(); await page.goto('/fixture?utm_campaign=visit-only');
  await init(page, {sessionTimeoutMs: 60000, choice: {enabled: true, persistentAttribution: false}}); await flush(page);
  const before = (await events(request))[0];
  await page.clock.fastForward(61000); await page.evaluate(() => history.pushState({}, '', '/next')); await flush(page);
  const after = (await events(request)).at(-1);
  expect(after.visitor_id).toBe(before.visitor_id); expect(after.visit_id).not.toBe(before.visit_id); expect(after.touch_id).toBeUndefined();
});

test('UTM medium alone does not create a nonexistent touch reference', async ({page, request}) => {
  await page.goto('/fixture?utm_medium=social'); await init(page); await page.locator('#button').click(); await flush(page);
  const collected = await events(request);
  expect(collected[0].campaign).toEqual({utm_medium: 'social'});
  expect(collected[1].touch_id).toBeUndefined();
});

test('permanently lost landing does not poison later recovered observations with a missing touch', async ({page, request}) => {
  await request.post('/test/config', {data: {status: 503}});
  await page.goto('/fixture?utm_campaign=lost'); await init(page, {queue: {maxAttempts: 1, flushIntervalMs: 60000}}); await flush(page);
  expect((await page.evaluate(() => (window as any).client.getJourneyContext())).touch_id).toBeNull();
  await request.post('/test/config', {data: {status: 0}}); await page.locator('#button').click(); await flush(page);
  expect((await events(request)).at(-1).touch_id).toBeUndefined();
  expect((await coverage(page)).accepted).toBe(1);
});
