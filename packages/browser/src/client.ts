import {BrowserState, newId} from './state.js';
import {DeliveryQueue} from './queue.js';
import {campaignFrom, propertiesFrom, safeName, sanitizeURL} from './sanitize.js';
import {observeNavigation} from './navigation.js';
import type {AnalyticsClient, CollectionChoice, Coverage, Observation, Options, Properties, Transport} from './types.js';
export interface ServiceAdapter extends Transport {
  identify(evidence: string, state: BrowserState, signal: AbortSignal): Promise<void>;
  reset(visitId?: string, visitorId?: string): Promise<void>;
  withdraw(visitorId?: string, visitId?: string): Promise<void>;
  handoff(destination: string, state: BrowserState, signal: AbortSignal): Promise<string>;
  redeem(reference: string, state: BrowserState, signal: AbortSignal): Promise<void>;
}
export function createClient(options: Options, adapter: ServiceAdapter, onShutdown: () => void, crossPropertyAllowed = false, initialURL = location.href): AnalyticsClient {
  const navigatorWithSignals = navigator as Navigator & {globalPrivacyControl?: boolean};
  const coverage: Coverage = {runtimeId: newId(), coverageStartedAt: new Date().toISOString(), controlFailures: 0, produced: 0, accepted: 0, rejected: 0, dropped: {queue: 0, oversized: 0, exhausted: 0, policy: 0, invalid: 0, notReady: 0}, queuedEvents: 0, queuedBytes: 0, inFlightEvents: 0, inFlightBytes: 0, highWaterEvents: 0, highWaterBytes: 0, requests: 0, requestBytes: 0, preferenceBlocked: options.respectPreferenceSignals !== false && (navigatorWithSignals.globalPrivacyControl === true || navigator.doNotTrack === '1'), storageUnavailable: false};
  const state = new BrowserState(options, coverage, crossPropertyAllowed);
  const queue = new DeliveryQueue(adapter, coverage, options.queue, event => state.forgetTouch(event.id));
  const routes = options.routes ?? ['/'];
  const removals: (() => void)[] = [];
  let stopped = false;
  let latestURL = '';
  let epoch = 0;
  const operations = new Set<AbortController>();
  const getCoverage = (): Coverage => ({...coverage, dropped: {...coverage.dropped}});
  const referrer = sanitizeURL(document.referrer, [], location.href);
  const page = () => sanitizeURL(location.href, routes, location.href)!;
  const emit = (name: string, category: Observation['category'], properties: Properties = {}, active = false, campaign?: Observation['campaign'], sourceURL = location.href, eventId?: string) => {
    if (stopped || !state.enabled) return false;
    if (active && document.visibilityState === 'visible') state.activity();
    state.expire();
    coverage.produced++;
    const event: Observation = {
      id: eventId ?? newId(), name, category, occurredAt: new Date().toISOString(), properties,
      page: sanitizeURL(sourceURL, routes, location.href)!, referrer, profile: state.profile, policyVersion: options.policyVersion,
      coverage: getCoverage(),
      ...(state.visitId ? {visitId: state.visitId} : {}),
      ...(state.visitorId ? {visitorId: state.visitorId} : {}),
      ...(state.firstTouch ? {firstTouch: state.firstTouch} : {}),
      ...(state.lastTouch ? {lastTouch: state.lastTouch} : {}),
      ...(campaign ? {campaign} : {})
    };
    const queued = queue.push(event);
    if (!queued) state.forgetTouch(event.id);
    return queued;
  };
  const navigation = (sourceURL = location.href, eventId?: string) => {
    if (stopped || latestURL === sourceURL) return;
    latestURL = sourceURL;
    if (!state.enabled) return;
    if (document.visibilityState === 'visible') state.activity();
    const campaign = campaignFrom(sourceURL, options.experimentKey ?? 'experiment');
    const touch = campaign && (campaign.utm_campaign || campaign.utm_source || campaign.experiment) ? state.touch(campaign, sanitizeURL(sourceURL, routes, location.href)!, referrer) : undefined;
    emit('page_view', 'navigation', {}, false, campaign, sourceURL, touch?.id);
  };
  const listen = (target: EventTarget, name: string, handler: (event: Event) => void) => {
    target.addEventListener(name, handler, {capture: true, passive: true});
    removals.push(() => target.removeEventListener(name, handler, true));
  };
  const elements = (event: Event): Element[] => {
    const path = event.composedPath();
    if (path.length > 64) return [];
    const elements = path.filter((element): element is Element => element instanceof Element);
    if (elements.some(element => element.hasAttribute('data-analytics-ignore') || element.hasAttribute('contenteditable'))) return [];
    return elements;
  };
  const controlProperties = (element: Element): Properties => {
    const role = element.getAttribute('role');
    const category = role && ['button', 'link', 'tab', 'menuitem', 'menuitemcheckbox', 'menuitemradio', 'checkbox', 'radio', 'switch', 'combobox', 'dialog'].includes(role) ? role : element.localName;
    const control = element.getAttribute('data-analytics');
    const props: Properties = {control_kind: category};
    if (safeName(control)) props.control = control;
    if (element instanceof HTMLAnchorElement) {
      const destination = sanitizeURL(element.href, routes, location.href);
      if (destination) props.destination = destination;
    }
    return props;
  };
  const selector = 'a[href],button,summary,input[type=submit],input[type=button],input[type=reset],[role=button],[role=link],[role=tab],[role=menuitem],[role=menuitemcheckbox],[role=menuitemradio],[role=checkbox],[role=radio],[role=switch],[role=combobox]';
  const click = (event: Event) => {
    if (event instanceof MouseEvent && (event.type === 'auxclick' ? event.button !== 1 : event.button !== 0)) return;
    const element = elements(event).find(element => element.matches(selector));
    if (element) emit('control_activate', 'interaction', controlProperties(element), true);
  };
  listen(document, 'click', click); listen(document, 'auxclick', click);
  listen(document, 'submit', event => {
    const form = elements(event).find(element => element instanceof HTMLFormElement);
    if (form instanceof HTMLFormElement) emit('form_submit', 'form', {...controlProperties(form), method: form.method.toLowerCase() === 'post' ? 'post' : 'get'}, true);
  });
  const invalid = new Map<Element, Set<string>>();
  let invalidScheduled = false;
  listen(document, 'invalid', event => {
    const input = elements(event)[0];
    if (!(input instanceof HTMLInputElement || input instanceof HTMLTextAreaElement || input instanceof HTMLSelectElement)) return;
    const owner = input.form ?? input;
    if (invalid.size >= 20 && !invalid.has(owner)) return;
    const validity = input.validity;
    const kind = validity.valueMissing ? 'required' : validity.typeMismatch || validity.patternMismatch ? 'format' : validity.rangeOverflow || validity.rangeUnderflow || validity.stepMismatch ? 'range' : validity.tooLong || validity.tooShort ? 'length' : validity.customError ? 'custom' : 'invalid';
    const kinds = invalid.get(owner) ?? new Set<string>(); kinds.add(kind); invalid.set(owner, kinds);
    if (!invalidScheduled) {
      invalidScheduled = true;
      queueMicrotask(() => {
        for (const [element, kinds] of invalid) emit('form_validation', 'form', {...controlProperties(element), result: kinds.size === 1 ? [...kinds][0]! : 'multiple'}, true);
        invalid.clear(); invalidScheduled = false;
      });
    }
  });
  listen(document, 'toggle', event => {
    const element = elements(event)[0];
    if (element instanceof HTMLDetailsElement) emit('disclosure_change', 'interaction', {...controlProperties(element), open: element.open}, true);
    else if (element instanceof HTMLDialogElement) emit('dialog_change', 'interaction', {...controlProperties(element), open: element.open});
  });
  listen(document, 'close', event => {
    const element = elements(event)[0];
    if (element instanceof HTMLDialogElement) emit('dialog_close', 'interaction', controlProperties(element));
  });
  listen(document, 'visibilitychange', () => {
    if (options.captureVisibility) emit('visibility_change', 'lifecycle', {visibility: document.visibilityState});
    if (document.visibilityState === 'hidden') { state.save(); void queue.flush(true); }
  });
  listen(window, 'pagehide', () => { state.save(); void queue.flush(true); });
  listen(window, 'pageshow', event => {
    if ((event as PageTransitionEvent).persisted && options.captureVisibility) emit('page_restore', 'lifecycle');
  });
  removals.push(observeNavigation(navigation));
  navigation(initialURL);
  if (initialURL !== location.href) navigation();
  const invalidate = () => {
    epoch++;
    for (const controller of operations) controller.abort();
    operations.clear();
    queue.clear(); invalid.clear(); state.reset();
  };
  listen(window, 'storage', event => {
    const change = event as StorageEvent;
    if ((change.key === state.storageKey || change.key === null) && change.newValue === null && state.visitorId) invalidate();
  });
  const operation = async <T>(fn: (signal: AbortSignal) => Promise<T>): Promise<T> => {
    const controller = new AbortController(); operations.add(controller);
    const timeout = setTimeout(() => controller.abort(), options.queue?.requestTimeoutMs ?? 5000);
    const version = epoch;
    try {
      const result = await fn(controller.signal);
      if (version !== epoch || stopped) throw new Error('Collection changed during operation');
      return result;
    } finally { clearTimeout(timeout); operations.delete(controller); }
  };
  const requireJourney = () => {
    if (stopped || !state.enabled || state.profile === 'minimal' || !state.choice.crossProperty) throw new Error('Cross-property collection is disabled');
    state.activity();
  };
  const client: AnalyticsClient = {
    ready: Promise.resolve(),
    track(name, properties = {}) {
      const fields = options.schemas?.[name];
      if (!safeName(name) || !fields || /^(page_view|control_activate|form_submit|form_validation|dialog_change|dialog_close|disclosure_change|visibility_change|page_restore|signup|enrollment|human_action|scheduled|system)$/.test(name)) { coverage.dropped.invalid++; return false; }
      return emit(name, 'custom', state.choice.detailedProperties ? propertiesFrom(properties, fields) : {});
    },
    async identify(evidence) {
      if (stopped || !state.enabled || state.profile !== 'identified') throw new Error('Identified collection is disabled');
      if (typeof evidence !== 'string' || evidence.length < 16 || evidence.length > 2048 || !/^[A-Za-z0-9._~-]+$/.test(evidence)) throw new Error('Expected a server-issued opaque proof');
      // Switching accounts is a hard boundary even if the new proof is rejected.
      if (state.identityEvidence) { invalidate(); throw new Error('Account switch requires a new proof bound to fresh journey identifiers'); }
      state.activity();
      await operation(signal => adapter.identify(evidence, state, signal));
      state.identityEvidence = 'verified';
    },
    reset() {
      const visit = state.visitId; const visitor = state.visitorId;
      invalidate();
      void adapter.reset(visit, visitor).catch(() => { coverage.controlFailures++; });
    },
    setCollectionChoice(choice) {
      const visitor = state.visitorId; const visit = state.visitId;
      invalidate(); state.setChoice(choice);
      if (!state.enabled) void adapter.withdraw(visitor, visit).catch(() => { coverage.controlFailures++; });
      // Re-enabling creates a fresh observed landing, never links purged history.
      if (state.enabled) { latestURL = ''; navigation(); }
    },
    flush: () => queue.flush(),
    async shutdown() {
      if (stopped) return;
      stopped = true; epoch++;
      for (const controller of operations) controller.abort();
      for (const remove of removals) remove();
      onShutdown();
      await queue.shutdown();
    },
    getCoverage,
    getJourneyContext: () => ({property_id: options.propertyId, visit_id: state.visitId ?? null, visitor_id: state.visitorId ?? null, touch_id: state.firstTouch?.id ?? null}),
    async createHandoff(destination) {
      requireJourney();
      const url = new URL(destination);
      if (!options.handoffOrigins?.[url.origin] || (url.protocol !== 'https:' && !['localhost', '127.0.0.1'].includes(url.hostname))) throw new Error('Handoff destination is not allowlisted');
      await queue.flush();
      return operation(signal => adapter.handoff(url.origin, state, signal));
    },
    async redeemHandoff(reference) {
      requireJourney();
      if (typeof reference !== 'string' || reference.length < 16 || reference.length > 2048 || !/^[A-Za-z0-9._~-]+$/.test(reference)) throw new Error('Invalid handoff reference');
      await operation(signal => adapter.redeem(reference, state, signal));
    }
  };
  return client;
}
