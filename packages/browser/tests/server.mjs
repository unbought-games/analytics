import {createServer} from 'node:http';
import {readFile} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
export const propertyId = '11111111-1111-4111-8111-111111111111';
export const audienceId = '22222222-2222-4222-8222-222222222222';
export function fixtureServer() {
  let state = {requests: [], config: {}, proofs: {}};
  const html = `<!doctype html><meta charset="utf-8"><title>Synthetic browser fixture</title>
  <button id="button" data-analytics="start">Never collect this private text</button>
  <a id="link" href="/next?password=hidden#secret">Link</a>
  <div role="tab" id="tab" data-analytics="details">Tab</div>
  <div role="menuitem" id="menu">Menu</div>
  <div data-analytics-ignore><button id="excluded">Excluded</button></div>
  <div contenteditable><button id="editable">Private editor</button></div>
  <form id="form" data-analytics="registration"><input id="email" type="email" required value="synthetic@example.invalid"><input id="required" required><button id="submit">Submit</button></form>
  <details id="details"><summary>Details</summary><p>Content</p></details>
  <dialog id="dialog" data-analytics="help">Dialog contents</dialog>
  <div id="shadow"></div><script src="/dist/analytics.global.js"></script>
  <script>document.querySelector('#form').addEventListener('submit', e => e.preventDefault());
  document.querySelector('#link').addEventListener('click', e => e.preventDefault());
  document.querySelector('#shadow').attachShadow({mode:'open'}).innerHTML='<button data-analytics="shadow_control">Shadow private text</button>';</script>`;
  const server = createServer(async (req, res) => {
    const origin = req.headers.origin;
    res.setHeader('Access-Control-Allow-Origin', origin ?? '*');
    res.setHeader('Access-Control-Allow-Headers', 'content-type,x-property-token');
    res.setHeader('Access-Control-Allow-Methods', 'GET,POST,OPTIONS');
    if (req.method === 'OPTIONS') { res.end(); return; }
    const path = new URL(req.url, 'http://localhost').pathname;
    const json = (value, code = 200) => { res.statusCode = code; res.setHeader('Content-Type', 'application/json'); res.end(JSON.stringify(value)); };
    if (path === '/dist/analytics.global.js') { res.setHeader('Content-Type', 'text/javascript'); res.end(await readFile(new URL('../dist/analytics.global.js', import.meta.url))); return; }
    let body;
    if (req.method === 'POST') { const chunks = []; for await (const chunk of req) chunks.push(chunk); body = JSON.parse(Buffer.concat(chunks).toString() || '{}'); }
    if (path === '/test/reset') { state = {requests: [], config: {}, proofs: {}}; return json({}); }
    if (path === '/test/state') return json(state);
    if (path === '/test/config') { Object.assign(state.config, body); return json({}); }
    if (path === '/test/proof') { const proof = 'synthetic-proof-' + crypto.randomUUID(); state.proofs[proof] = body; return json({proof}); }
    if (path.startsWith('/v1/')) {
      state.requests.push({path, body, token: req.headers['x-property-token'], referrer: req.headers.referer ?? null});
      if (path === '/v1/ingest/policy') {
        if (state.config.policyDelay) await new Promise(resolve => setTimeout(resolve, state.config.policyDelay));
        if (state.config.policyFailure) return json({}, 503);
        return json({property_id: propertyId, policy: {profile: state.config.profile ?? 'identified', version: 1, cross_property: state.config.crossProperty ?? true, respect_global_privacy_control: true, attribution_window_days: 30, allowed_query_keys: ['utm_source','utm_medium','utm_campaign','utm_content','utm_term','experiment'], effective_at: '2026-01-01T00:00:00Z', event_retention_days: 90, identity_retention_days: 365}, handoff_audiences: [audienceId], schemas: [], limitations: []});
      }
      if (path === '/v1/ingest/browser') {
        if (state.config.delay) await new Promise(resolve => setTimeout(resolve, state.config.delay));
        if (state.config.status) return json({}, state.config.status);
        if (state.config.badAck) return json({results: []});
        return json({committed_at: new Date().toISOString(), results: body.events.map((event, index) => ({event_id: event.event_id, status: state.config.reject && index === 0 ? 'rejected' : state.config.duplicate ? 'duplicate' : 'accepted', receipt_cursor: '9007199254740993'}))});
      }
      if (path === '/v1/identity/identify') {
        const proof = state.proofs[body.proof];
        if (!proof || ['property_id', 'visit_id', 'visitor_id'].some(key => proof[key] !== body[key])) return json({}, 403);
        delete state.proofs[body.proof];
        return json({identified: true});
      }
      if (path === '/v1/journey/handoff') return json({handoff: 'synthetic-handoff-' + crypto.randomUUID(), expires_at: new Date(Date.now() + 60000).toISOString()});
      return json({});
    }
    res.setHeader('Content-Type', 'text/html'); res.end(html);
  });
  return server;
}
if (process.argv[1] === fileURLToPath(import.meta.url)) fixtureServer().listen(41739, '127.0.0.1');
