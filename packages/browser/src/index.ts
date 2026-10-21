import {createClient} from './client.js';
import {HttpAdapter} from './transport.js';
import {boundedText} from './sanitize.js';
import type {AnalyticsClient, CollectionChoice, Coverage, Options} from './types.js';
export type {AnalyticsClient, CollectionChoice, Coverage, Field, Options, Profile, Properties} from './types.js';
const clientsKey = Symbol.for('independent-analytics.clients.v1');
type Registry = typeof globalThis & {[clientsKey]?: Map<string, AnalyticsClient>};
export function init(options: Options): AnalyticsClient {
  if (typeof window === 'undefined' || typeof document === 'undefined') throw new Error('Initialize analytics in a browser lifecycle');
  const endpoint = new URL(options.endpoint);
  if (endpoint.username || endpoint.password || endpoint.search || endpoint.hash || (endpoint.protocol !== 'https:' && !(endpoint.protocol === 'http:' && ['localhost', '127.0.0.1', '[::1]'].includes(endpoint.hostname)))) throw new Error('Analytics endpoint requires HTTPS (HTTP is allowed on loopback)');
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(options.propertyId) || !Number.isSafeInteger(options.policyVersion) || options.policyVersion < 1) throw new Error('Invalid property or policy version');
  if (!options.publicToken || options.publicToken.length > 512 || /[\r\n]/.test(options.publicToken)) throw new Error('Expected a public property token');
  if (options.profile && !['minimal', 'journey', 'identified'].includes(options.profile)) throw new Error('Invalid collection profile');
  if (options.routes && (options.routes.length > 200 || options.routes.some(route => !/^\/(?:[a-zA-Z0-9_:-]+\/?)*$/.test(route) || route.length > 256))) throw new Error('Routes must contain static segments or named parameters');
  if (options.environment && !boundedText(options.environment, 64) || options.release && !boundedText(options.release, 128)) throw new Error('Invalid environment or release');
  for (const value of [...Object.values(options.queue ?? {}), options.sessionTimeoutMs, options.attributionWindowMs]) if (value !== undefined && (!Number.isFinite(value) || value <= 0)) throw new Error('Limits must be positive finite numbers');
  const registry = globalThis as Registry;
  const clients = registry[clientsKey] ??= new Map();
  const key = `${endpoint.href.replace(/\/$/, '')}:${options.propertyId}`;
  const existing = clients.get(key); if (existing) return existing;
  const initialURL = location.href;
  const adapter = new HttpAdapter({...options});
  let client: AnalyticsClient | undefined;
  let stopped = false; let reset = false; let earlyDrops = 0;
  let pendingChoice: CollectionChoice | undefined;
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), Math.min(options.queue?.requestTimeoutMs ?? 5000, 30000));
  const ready = adapter.policy(controller.signal).then(({options: effective, crossProperty}) => {
    if (stopped) return;
    // Server ceiling remains in effective.profile; a subsequent choice cannot escalate it.
    if (pendingChoice) effective.choice = {...pendingChoice, crossProperty: pendingChoice.crossProperty === true && crossProperty};
    client = createClient(effective, adapter, () => clients.delete(key), crossProperty, initialURL);
    if (reset) client.reset();
  }).finally(() => clearTimeout(timeout));
  // Consumers can await ready for errors; avoid an unhandled rejection on passive static sites.
  void ready.catch(() => {});
  const requireClient = async () => { await ready; if (!client || stopped) throw new Error('Analytics client is stopped'); return client; };
  const proxy: AnalyticsClient = {
    ready,
    track(name, properties) { if (!client) { earlyDrops++; return false; } return client.track(name, properties); },
    async identify(evidence) { await (await requireClient()).identify(evidence); },
    reset() { if (client) client.reset(); else reset = true; },
    setCollectionChoice(choice) { if (client) client.setCollectionChoice(choice); else pendingChoice = {...choice}; },
    async flush() { await (await requireClient()).flush(); },
    async shutdown() { stopped = true; controller.abort(); clients.delete(key); await client?.shutdown(); },
    getJourneyContext() { return client?.getJourneyContext() ?? {property_id: options.propertyId, visit_id: null, visitor_id: null, touch_id: null}; },
    getCoverage(): Coverage {
      if (client) { const coverage = client.getCoverage(); coverage.dropped.notReady += earlyDrops; return coverage; }
      return {runtimeId: '', coverageStartedAt: null, controlFailures: 0, produced: 0, accepted: 0, rejected: 0, dropped: {queue: 0, oversized: 0, exhausted: 0, policy: 0, invalid: 0, notReady: earlyDrops}, queuedEvents: 0, queuedBytes: 0, inFlightEvents: 0, inFlightBytes: 0, highWaterEvents: 0, highWaterBytes: 0, requests: 0, requestBytes: 0, preferenceBlocked: false, storageUnavailable: false};
    },
    async createHandoff(destination) { return (await requireClient()).createHandoff(destination); },
    async redeemHandoff(reference) { await (await requireClient()).redeemHandoff(reference); }
  };
  clients.set(key, proxy);
  return proxy;
}
export {browserSchemas} from './schemas.js';
