import type {ServiceAdapter} from './client.js';
import type {BrowserState} from './state.js';
import type {BatchResult, Observation, Options} from './types.js';
import type {CollectionPolicy, EventEnvelope, HandoffRequest, HandoffResponse, IdentityResetRequest, IngestAcknowledgement, IngestBatch} from './wire.js';
import {encodedBytes} from './queue.js';
const profiles = ['minimal', 'journey', 'identified'];
async function readJSON(response: Response): Promise<unknown> {
  const reader = response.body?.getReader();
  if (!reader) throw new Error('Empty analytics response');
  const chunks: Uint8Array[] = []; let size = 0;
  try {
    while (true) {
      const {done, value} = await reader.read();
      if (done) break;
      size += value.length;
      if (size > 65536) throw new Error('Analytics response exceeds limit');
      chunks.push(value);
    }
  } finally { await reader.cancel().catch(() => {}); }
  const bytes = new Uint8Array(size); let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
  return JSON.parse(new TextDecoder().decode(bytes));
}
export class HttpAdapter implements ServiceAdapter {
  private reportedDrops = 0;
  private envelopes = new WeakMap<Observation, EventEnvelope>();
  constructor(private options: Options) {}
  private request(path: string, body: unknown | undefined, signal: AbortSignal, keepalive = false) {
    return fetch(`${this.options.endpoint.replace(/\/$/, '')}${path}`, {
      method: body === undefined ? 'GET' : 'POST', headers: {'X-Property-Token': this.options.publicToken, ...(body === undefined ? {} : {'Content-Type': 'application/json'})},
      ...(body === undefined ? {} : {body: JSON.stringify(body)}), signal, keepalive,
      credentials: 'omit', redirect: 'error', referrerPolicy: 'no-referrer', cache: 'no-store', mode: 'cors'
    });
  }
  async policy(signal: AbortSignal): Promise<{options: Options; crossProperty: boolean}> {
    const response = await this.request(`/v1/ingest/policy?property_id=${encodeURIComponent(this.options.propertyId)}`, undefined, signal);
    if (!response.ok) throw new Error('Analytics policy unavailable');
    const result = await readJSON(response) as {property_id: string; policy: CollectionPolicy; handoff_audiences: string[]};
    const policy = result?.policy;
    if (result?.property_id !== this.options.propertyId || !policy || !profiles.includes(policy.profile) || !Number.isSafeInteger(policy.version) || policy.version < 1 || typeof policy.cross_property !== 'boolean' || typeof policy.respect_global_privacy_control !== 'boolean' || !Number.isSafeInteger(policy.attribution_window_days) || policy.attribution_window_days < 1) throw new Error('Invalid analytics policy');
    const profile = profiles.indexOf(this.options.profile ?? 'minimal') <= profiles.indexOf(policy.profile) ? this.options.profile ?? 'minimal' : policy.profile;
    const choice = {...this.options.choice ?? {enabled: true}, crossProperty: this.options.choice?.crossProperty === true && policy.cross_property};
    const origins = Object.fromEntries(Object.entries(this.options.handoffOrigins ?? {}).filter(([, audience]) => result.handoff_audiences?.includes(audience)));
    this.options = {...this.options, profile, choice, handoffOrigins: origins, policyVersion: policy.version, respectPreferenceSignals: policy.respect_global_privacy_control || this.options.respectPreferenceSignals !== false, attributionWindowMs: Math.min(this.options.attributionWindowMs ?? 2592000000, policy.attribution_window_days * 86400000)};
    return {options: this.options, crossProperty: policy.cross_property};
  }
  private envelope(event: Observation): EventEnvelope {
    let envelope = this.envelopes.get(event);
    if (!envelope) {
      const {control, ...properties} = event.properties;
      const {experiment, ...campaign} = event.campaign ?? {};
      envelope = {
        event_id: event.id, property_id: this.options.propertyId, name: event.name, schema_version: 1,
        producer: 'browser', category: event.category === 'lifecycle' ? 'visibility' : event.category,
        occurred_at: event.occurredAt, clock: 'client',
        context: {environment: this.options.environment ?? 'default', page: event.page, referrer: event.referrer,
          ...(typeof control === 'string' ? {control} : {}), ...(this.options.release ? {release: this.options.release} : {})},
        collection: {profile: event.profile, policy_version: event.policyVersion, consent: true, sample_rate: 1,
          dropped_events: Math.max(0, Object.values(event.coverage.dropped).reduce((sum, count) => sum + count, 0) - this.reportedDrops)},
        properties: {...properties},
        ...(event.visitId ? {visit_id: event.visitId} : {}), ...(event.visitorId ? {visitor_id: event.visitorId} : {}),
        ...(event.campaign ? {campaign: {...campaign, ...(experiment ? {experiment_id: experiment} : {})}} : {}),
        ...(event.firstTouch && !event.campaign ? {touch_id: event.firstTouch.id} : {})
      };
      this.envelopes.set(event, envelope);
    }
    return envelope;
  }
  enqueued(event: Observation) { this.reportedDrops = Object.values(event.coverage.dropped).reduce((sum, count) => sum + count, 0); }
  bytes(event: Observation) { return encodedBytes(this.envelope(event)); }
  async send(events: readonly Observation[], signal: AbortSignal, keepalive: boolean): Promise<BatchResult> {
    const batch: IngestBatch = {events: events.map(event => this.envelope(event))};
    const response = await this.request('/v1/ingest/browser', batch, signal, keepalive);
    if (!response.ok) {
      if ([408, 425, 429].includes(response.status) || response.status >= 500) throw new Error('Analytics ingestion temporarily unavailable');
      return {accepted: [], rejected: events.map(event => ({id: event.id, retryable: false}))};
    }
    const ack = await readJSON(response) as IngestAcknowledgement;
    if (!Array.isArray(ack?.results) || ack.results.length > events.length) throw new Error('Invalid analytics acknowledgement');
    const accepted: string[] = []; const rejected: BatchResult['rejected'] = [];
    for (const result of ack.results) {
      if (!result || typeof result.event_id !== 'string') throw new Error('Invalid analytics acknowledgement');
      if (result.status === 'accepted' || result.status === 'duplicate') accepted.push(result.event_id);
      else rejected.push({id: result.event_id, retryable: !['rejected', 'conflict'].includes(result.status)});
    }
    return {accepted, rejected};
  }
  async identify(evidence: string, state: BrowserState, signal: AbortSignal) {
    const response = await this.request('/v1/identity/identify', {proof: evidence, property_id: this.options.propertyId, visit_id: state.visitId ?? null, visitor_id: state.visitorId ?? null}, signal);
    if (!response.ok) throw new Error('Identity proof was not accepted');
  }
  async reset(visitId?: string, visitorId?: string) { await this.control('/v1/identity/reset', visitId, visitorId); }
  async withdraw(visitorId?: string, visitId?: string) { await this.control('/v1/privacy/withdraw', visitId, visitorId); }
  private async control(path: string, visitId?: string, visitorId?: string) {
    if (!visitId && !visitorId) return;
    const request: IdentityResetRequest = {property_id: this.options.propertyId, visit_id: visitId ?? null, visitor_id: visitorId ?? null};
    const response = await this.request(path, request, AbortSignal.timeout(5000), true);
    if (!response.ok) throw new Error('Analytics control request failed');
  }
  async handoff(destination: string, state: BrowserState, signal: AbortSignal): Promise<string> {
    const audience = this.options.handoffOrigins?.[destination];
    if (!audience) throw new Error('Handoff audience is not configured');
    const request: HandoffRequest = {property_id: this.options.propertyId, audience, visit_id: state.visitId ?? null, visitor_id: state.visitorId ?? null, touch_id: state.firstTouch?.id ?? null};
    const response = await this.request('/v1/journey/handoff', request, signal);
    if (!response.ok) throw new Error('Handoff was not issued');
    const result = await readJSON(response) as HandoffResponse;
    if (typeof result?.handoff !== 'string' || result.handoff.length < 16 || result.handoff.length > 2048 || !/^[A-Za-z0-9._~-]+$/.test(result.handoff) || !Number.isFinite(Date.parse(result.expires_at)) || Date.parse(result.expires_at) <= Date.now()) throw new Error('Invalid handoff response');
    return result.handoff;
  }
  async redeem(reference: string, state: BrowserState, signal: AbortSignal) {
    if (!this.options.redeemBridge) throw new Error('Configure an authenticated application redeemBridge');
    await this.options.redeemBridge({handoff: reference, audience: this.options.propertyId, visit_id: state.visitId ?? null, visitor_id: state.visitorId ?? null}, signal);
  }
}
