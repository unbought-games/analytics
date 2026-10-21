import type {Campaign, Field, Properties} from './types.js';
const namePattern = /^[a-z][a-z0-9_.-]{0,63}$/;
const forbidden = /password|passwd|secret|token|authorization|cookie|email|phone|address|credit|card|field.?value/i;
export const safeName = (value: unknown): value is string => typeof value === 'string' && namePattern.test(value) && !forbidden.test(value);
export function boundedText(value: unknown, max = 128): string | undefined {
  if (typeof value !== 'string' || !value || value.length > max || /[\u0000-\u001f\u007f-\u009f\ufffd<>@\\]/u.test(value)) return;
  // Exclude obvious credentials and address-like content even in allowlisted fields.
  if (/(?:bearer\s|eyJ[A-Za-z0-9_-]{12}|[a-f0-9]{32,}|\b\d{10,}\b)/i.test(value)) return;
  return value.normalize('NFC');
}
export function campaignFrom(url: string, experimentKey: string): Campaign | undefined {
  const params = new URL(url).searchParams;
  const result: Campaign = {};
  for (const key of ['utm_source', 'utm_medium', 'utm_campaign', 'utm_term', 'utm_content'] as const) {
    const value = params.getAll(key);
    if (value.length === 1) {
      const safe = boundedText(value[0]);
      if (safe) result[key] = safe;
    }
  }
  const values = params.getAll(experimentKey);
  const experiment = values.length === 1 ? boundedText(values[0]) : undefined;
  if (experiment) result.experiment = experiment;
  return Object.keys(result).length ? result : undefined;
}
export function sanitizeURL(raw: string, routes: readonly string[], base: string): string | null {
  try {
    const url = new URL(raw, base);
    if (!['http:', 'https:'].includes(url.protocol)) return null;
    let path = '/[redacted]';
    for (const route of routes) {
      const parts = route.split('/');
      const actual = url.pathname.split('/');
      if (parts.length !== actual.length) continue;
      if (parts.every((part, index) => part.startsWith(':') ? Boolean(actual[index]) : part === actual[index])) {
        path = parts.map(part => part.startsWith(':') ? ':redacted' : part).join('/');
        break;
      }
    }
    return `${url.origin}${path}`;
  } catch { return null; }
}
export function propertiesFrom(input: Properties, fields: Record<string, Field>): Properties {
  const result: Properties = {};
  // Inspect only declared keys. Never enumerate a potentially huge untrusted object.
  for (const key of Object.keys(fields).slice(0, 20)) {
    if (!safeName(key)) continue;
    const value = input[key];
    const field = fields[key]!;
    if (field.type === 'string') {
      const safe = boundedText(value, Math.min(field.maxLength ?? 128, 256));
      if (safe && (!field.values || field.values.includes(safe))) result[key] = safe;
    } else if ((field.type === 'number' || field.type === 'integer') && typeof value === 'number' && Number.isFinite(value) && (field.type !== 'integer' || Number.isSafeInteger(value))) result[key] = value;
    else if (field.type === 'boolean' && typeof value === 'boolean') result[key] = value;
  }
  return result;
}
