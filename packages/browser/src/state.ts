import type {Campaign, CollectionChoice, Coverage, Options, Profile, Touch} from './types.js';
import {boundedText, sanitizeURL} from './sanitize.js';
const rank = {minimal: 0, journey: 1, identified: 2};
const uuidPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const id = (value: unknown): value is string => typeof value === 'string' && uuidPattern.test(value);
export const newId = () => crypto.randomUUID();
export class BrowserState {
  choice: CollectionChoice;
  profile: Profile;
  visitId: string | undefined;
  visitorId: string | undefined;
  identityEvidence: string | undefined;
  firstTouch: Touch | undefined;
  lastTouch: Touch | undefined;
  private lastActive = 0;
  private savedAt = 0;
  private key;
  readonly timeout;
  readonly attributionWindow;
  constructor(private options: Options, private coverage: Coverage, private crossPropertyAllowed = false) {
    this.key = `analytics:v1:${new URL(options.endpoint).origin}:${options.propertyId}`;
    this.choice = {...options.choice ?? {enabled: true}, crossProperty: options.choice?.crossProperty === true && crossPropertyAllowed};
    this.profile = this.effectiveProfile();
    this.timeout = Math.min(86400000, Math.max(60000, options.sessionTimeoutMs ?? 1800000));
    this.attributionWindow = Math.min(2592000000, Math.max(60000, options.attributionWindowMs ?? 2592000000));
    if (this.enabled && this.profile !== 'minimal') this.restore();
    else this.erase();
  }
  get storageKey() { return this.key; }
  get enabled() { return this.choice.enabled && !this.coverage.preferenceBlocked; }
  effectiveProfile(): Profile {
    const ceiling = this.options.profile ?? 'minimal';
    const desired = this.choice.profile ?? ceiling;
    return rank[desired] <= rank[ceiling] ? desired : ceiling;
  }
  private storage(kind: 'localStorage' | 'sessionStorage', action: (storage: Storage) => void) {
    try { action(window[kind]); } catch { this.coverage.storageUnavailable = true; }
  }
  private restore() {
    const read = (kind: 'localStorage' | 'sessionStorage') => {
      this.storage(kind, storage => {
        const raw = storage.getItem(this.key);
        if (!raw || raw.length > 8192) return;
        try {
          const value = JSON.parse(raw) as Record<string, unknown>;
          if (kind === 'localStorage' && this.profile === 'identified' && id(value.visitorId) && typeof value.expiresAt === 'number' && value.expiresAt > Date.now() && value.expiresAt <= Date.now() + this.attributionWindow) this.visitorId = value.visitorId;
          if (kind === 'sessionStorage' && id(value.visitId) && typeof value.lastActive === 'number' && value.lastActive <= Date.now() && Date.now() - value.lastActive < this.timeout) {
            this.visitId = value.visitId; this.lastActive = value.lastActive;
          }
          if ((kind === 'sessionStorage' && this.visitId) || (kind === 'localStorage' && this.profile === 'identified' && this.choice.persistentAttribution)) {
            const first = this.validTouch(value.firstTouch); const last = this.validTouch(value.lastTouch);
            if (first && (!this.firstTouch || first.occurredAt < this.firstTouch.occurredAt)) this.firstTouch = first;
            if (last && (!this.lastTouch || last.occurredAt > this.lastTouch.occurredAt)) this.lastTouch = last;
          }
        } catch { storage.removeItem(this.key); }
      });
    };
    if (this.profile === 'identified') read('localStorage');
    else this.storage('localStorage', storage => storage.removeItem(this.key));
    read('sessionStorage');
    this.expire();
  }
  private validTouch(raw: unknown): Touch | undefined {
    if (!raw || typeof raw !== 'object') return;
    const touch = raw as Touch;
    const now = Date.now();
    if (!id(touch.id) || typeof touch.occurredAt !== 'string' || !Number.isFinite(Date.parse(touch.occurredAt)) || Date.parse(touch.occurredAt) > now || typeof touch.expiresAt !== 'number' || touch.expiresAt <= now || touch.expiresAt > Date.parse(touch.occurredAt) + this.attributionWindow) return;
    if (!touch.campaign || typeof touch.campaign !== 'object') return;
    const campaign: Campaign = {};
    for (const key of ['utm_source', 'utm_medium', 'utm_campaign', 'utm_term', 'utm_content', 'experiment'] as const) {
      const safe = boundedText(touch.campaign[key]); if (safe) campaign[key] = safe;
    }
    if (!Object.keys(campaign).length || typeof touch.landing !== 'string') return;
    const landing = sanitizeURL(touch.landing, this.options.routes ?? ['/'], location.href);
    const referrer = typeof touch.referrer === 'string' ? sanitizeURL(touch.referrer, [], location.href) : null;
    if (!landing) return;
    return {id: touch.id, occurredAt: touch.occurredAt, expiresAt: touch.expiresAt, campaign, landing, referrer};
  }
  expire() {
    if (this.firstTouch && this.firstTouch.expiresAt <= Date.now()) this.firstTouch = undefined;
    if (this.lastTouch && this.lastTouch.expiresAt <= Date.now()) this.lastTouch = undefined;
  }
  activity() {
    if (!this.enabled || this.profile === 'minimal') return;
    const now = Date.now();
    const fresh = !this.visitId || now - this.lastActive >= this.timeout;
    if (fresh) {
      this.visitId = newId();
      if (this.profile === 'journey' || !this.choice.persistentAttribution) { this.firstTouch = undefined; this.lastTouch = undefined; }
    }
    if (this.profile === 'identified' && !this.visitorId) this.visitorId = newId();
    this.lastActive = now;
    this.expire();
    if (fresh || now - this.savedAt >= 5000) this.save();
  }
  touch(campaign: Campaign, landing: string, referrer: string | null) {
    if (!this.enabled || this.profile === 'minimal') return;
    this.expire();
    const touch: Touch = {id: newId(), occurredAt: new Date().toISOString(), expiresAt: Date.now() + this.attributionWindow, campaign, landing, referrer};
    this.firstTouch ??= touch;
    this.lastTouch = touch;
    this.save();
    return touch;
  }
  forgetTouch(eventId: string) {
    let changed = false;
    if (this.firstTouch?.id === eventId) { this.firstTouch = undefined; changed = true; }
    if (this.lastTouch?.id === eventId) { this.lastTouch = undefined; changed = true; }
    if (changed) this.save();
  }
  save() {
    if (!this.enabled || this.profile === 'minimal') return;
    this.savedAt = Date.now();
    this.storage('sessionStorage', storage => storage.setItem(this.key, JSON.stringify({visitId: this.visitId, lastActive: this.lastActive, firstTouch: this.firstTouch, lastTouch: this.lastTouch})));
    if (this.profile === 'identified') this.storage('localStorage', storage => storage.setItem(this.key, JSON.stringify({visitorId: this.visitorId, expiresAt: Date.now() + this.attributionWindow, ...(this.choice.persistentAttribution ? {firstTouch: this.firstTouch, lastTouch: this.lastTouch} : {})})));
  }
  erase() {
    this.storage('localStorage', storage => storage.removeItem(this.key));
    this.storage('sessionStorage', storage => storage.removeItem(this.key));
  }
  reset() {
    this.visitId = undefined; this.visitorId = undefined; this.identityEvidence = undefined;
    this.firstTouch = undefined; this.lastTouch = undefined; this.lastActive = 0;
    this.erase();
  }
  setChoice(choice: CollectionChoice) {
    this.reset();
    this.choice = {...choice, crossProperty: choice.crossProperty === true && this.crossPropertyAllowed};
    this.profile = this.effectiveProfile();
  }
}
