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
export function sanitizeCampaign(input: Campaign | undefined): Campaign | undefined {
  if (!input || typeof input !== 'object') return;
  const result: Campaign = {};
  for (const key of ['utm_source', 'utm_medium', 'utm_campaign', 'utm_term', 'utm_content', 'experiment'] as const) {
    const value = boundedText(input[key]);
    if (value) result[key] = value;
  }
  return Object.keys(result).length ? result : undefined;
}
/** Query strings and fragments are excluded; route labels retain parameter names. */
export function pageContext(raw: string, routes: readonly string[], base: string): {page: string; route: string} | null {
  try {
    if (!raw) return null;
    const url = new URL(raw, base);
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password) return null;
    const actual = url.pathname.split('/');
    const compared = actual.length > 2 && actual.at(-1) === '' ? actual.slice(0, -1) : actual;
    // Specific static routes must win over equally shaped dynamic routes.
    const matches = routes.filter(route => {
      const parts = (route.replace(/\/$/, '') || '/').split('/');
      return parts.length === compared.length && parts.every((part, index) => part.startsWith(':') ? Boolean(actual[index]) : part === actual[index]);
    }).sort((a, b) => a.split(':').length - b.split(':').length);
    const route = matches[0] ?? url.pathname;
    const template = route.split('/');
    // Keep ordinary entity paths for drill-down, but never persist credentials.
    const path = actual.map((part, index) => /^:(?:code|token|secret|password)$/i.test(template[index] ?? '') ? template[index] : part).join('/');
    const result = {page: `${url.origin}${path}`, route: `${url.origin}${route}`};
    return result.page.length <= 2048 && result.route.length <= 2048 ? result : null;
  } catch { return null; }
}
export function sanitizeURL(raw: string, routes: readonly string[], base: string): string | null {
  return pageContext(raw, routes, base)?.page ?? null;
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
