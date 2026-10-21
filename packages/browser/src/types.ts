export type Profile = 'minimal' | 'journey' | 'identified';
export type Primitive = string | number | boolean | null;
export type Properties = Record<string, Primitive>;
export type Field = {type: 'string'; maxLength?: number; values?: readonly string[]} | {type: 'number' | 'integer'} | {type: 'boolean'};
export interface CollectionChoice {
  enabled: boolean;
  profile?: Profile;
  persistentAttribution?: boolean;
  crossProperty?: boolean;
  detailedProperties?: boolean;
}
export interface Campaign {
  utm_source?: string;
  utm_medium?: string;
  utm_campaign?: string;
  utm_term?: string;
  utm_content?: string;
  experiment?: string;
}
export interface Touch {
  id: string;
  occurredAt: string;
  expiresAt: number;
  campaign: Campaign;
  landing: string;
  referrer: string | null;
}
export interface Coverage {
  runtimeId: string;
  coverageStartedAt: string | null;
  controlFailures: number;
  produced: number;
  accepted: number;
  rejected: number;
  dropped: {queue: number; oversized: number; exhausted: number; policy: number; invalid: number; notReady: number};
  queuedEvents: number;
  queuedBytes: number;
  inFlightEvents: number;
  inFlightBytes: number;
  highWaterEvents: number;
  highWaterBytes: number;
  requests: number;
  requestBytes: number;
  preferenceBlocked: boolean;
  storageUnavailable: boolean;
}
/** Capture-domain observations, not the HTTP DTO. Only transport.ts serializes them. */
export interface Observation {
  id: string;
  name: string;
  occurredAt: string;
  category: 'navigation' | 'interaction' | 'form' | 'lifecycle' | 'custom';
  properties: Properties;
  page: string;
  referrer: string | null;
  profile: Profile;
  policyVersion: number;
  visitId?: string;
  visitorId?: string;
  identityEvidence?: string;
  firstTouch?: Touch;
  lastTouch?: Touch;
  campaign?: Campaign;
  coverage: Coverage;
}
export interface BatchResult {
  accepted: string[];
  rejected: {id: string; retryable: boolean}[];
}
/** Internal boundary used by the service adapter and controlled test fixtures. */
export interface Transport {
  enqueued?(event: Observation): void;
  bytes?(event: Observation): number;
  send(events: readonly Observation[], signal: AbortSignal, keepalive: boolean): Promise<BatchResult>;
}
export interface Options {
  /** Operator's base URL, e.g. https://analytics.example. */
  endpoint: string;
  propertyId: string;
  publicToken: string;
  profile?: Profile;
  policyVersion: number;
  choice?: CollectionChoice;
  /** Exact static paths or templates such as /articles/:slug. All parameters are redacted. */
  routes?: readonly string[];
  /** Origin allowlist for outgoing handoffs; only configured HTTPS audiences are eligible. */
  handoffOrigins?: Record<string, string>;
  /** Authenticated application forwards this exact DTO to /v1/journey/redeem. */
  redeemBridge?: (request: {handoff: string; audience: string; visit_id: string | null; visitor_id: string | null}, signal: AbortSignal) => Promise<void>;
  experimentKey?: string;
  schemas?: Record<string, Record<string, Field>>;
  captureVisibility?: boolean;
  respectPreferenceSignals?: boolean;
  sessionTimeoutMs?: number;
  attributionWindowMs?: number;
  environment?: string;
  release?: string;
  queue?: {maxEvents?: number; maxBytes?: number; batchSize?: number; batchBytes?: number; flushIntervalMs?: number; maxAttempts?: number; requestTimeoutMs?: number};
}
export interface AnalyticsClient {
  readonly ready: Promise<void>;
  track(name: string, properties?: Properties): boolean;
  /** A short-lived server-issued opaque proof; never a raw account ID. */
  identify(evidence: string): Promise<void>;
  reset(): void;
  setCollectionChoice(choice: CollectionChoice): void;
  flush(): Promise<void>;
  shutdown(): Promise<void>;
  getCoverage(): Coverage;
  getJourneyContext(): {property_id: string; visit_id: string | null; visitor_id: string | null; touch_id: string | null};
  /** Issue an opaque audience-bound handoff; no automatic link decoration. */
  createHandoff(destination: string): Promise<string>;
  redeemHandoff(reference: string): Promise<void>;
}
