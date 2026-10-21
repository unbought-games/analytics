// Generated from contracts/openapi.json; run npm run generate:wire in this package.
// Event IDs/timestamps are strings; no int64 receipts are converted to numbers.
export type Campaign = {
  "experiment_id"?: string | null;
  "utm_campaign"?: string | null;
  "utm_content"?: string | null;
  "utm_medium"?: string | null;
  "utm_source"?: string | null;
  "utm_term"?: string | null;
};

export type CollectionContext = {
  "consent": boolean;
  "dropped_events": number;
  "policy_version": number;
  "profile": CollectionProfile;
  "sample_rate": number;
};

export type CollectionPolicy = {
  "allowed_query_keys": Array<string>;
  "attribution_window_days": number;
  "cross_property": boolean;
  "effective_at": string;
  "event_retention_days": number;
  "identity_retention_days": number;
  "profile": CollectionProfile;
  "respect_global_privacy_control": boolean;
  "version": number;
};

export type CollectionProfile = "minimal" | "journey" | "identified";

export type EventAcknowledgement = {
  "code"?: string | null;
  "event_id": string;
  "receipt_cursor"?: string | null;
  "status": string;
};

export type EventCategory = "navigation" | "interaction" | "form" | "visibility" | "signup" | "enrollment" | "human_action" | "scheduled" | "system" | "custom";

export type EventContext = {
  "control"?: string | null;
  "correlation_id"?: string | null;
  "environment": string;
  "page"?: string | null;
  "referrer"?: string | null;
  "release"?: string | null;
};

export type EventEnvelope = {
  "attribution_rule"?: string | null;
  "campaign"?: Campaign | null;
  "category": EventCategory;
  "clock": string;
  "collection": CollectionContext;
  "context": EventContext;
  "event_id": string;
  "logical_time"?: string | null;
  "name": string;
  "occurred_at": string;
  "producer": Producer;
  "properties": Record<string, unknown>;
  "property_id": string;
  "schema_version": number;
  "subject"?: NamespacedId | null;
  "touch_id"?: string | null;
  "visit_id"?: string | null;
  "visitor_id"?: string | null;
};

export type EventSchemaDefinition = {
  "authoritative": boolean;
  "category": EventCategory;
  "name": string;
  "properties": Record<string, PropertyType>;
  "property_id": string;
  "version": number;
};

export type HandoffRequest = {
  "audience": string;
  "property_id": string;
  "touch_id"?: string | null;
  "visit_id"?: string | null;
  "visitor_id"?: string | null;
};

export type HandoffResponse = {
  "expires_at": string;
  "handoff": string;
};

export type IdentityResetRequest = {
  "property_id": string;
  "visit_id"?: string | null;
  "visitor_id"?: string | null;
};

export type IngestAcknowledgement = {
  "committed_at": string;
  "results": Array<EventAcknowledgement>;
};

export type IngestBatch = {
  "events": Array<EventEnvelope>;
};

export type NamespacedId = {
  "id": string;
  "namespace": string;
};

export type Producer = "browser" | "server";

export type PropertyType = {
  "kind": string;
  "max_length"?: number | null;
  "required": boolean;
  "unit"?: string | null;
};

export type RedeemHandoffRequest = {
  "audience": string;
  "handoff": string;
  "visit_id"?: string | null;
  "visitor_id"?: string | null;
};
