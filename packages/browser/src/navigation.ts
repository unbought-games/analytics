interface Hub {listeners: Set<() => void>; stop: () => void}
const hubKey = Symbol.for('independent-analytics.navigation.v1');
type Registry = typeof globalThis & {[hubKey]?: Hub};
export function observeNavigation(listener: () => void): () => void {
  const registry = globalThis as Registry;
  let hub = registry[hubKey];
  if (!hub) {
    const listeners = new Set<() => void>();
    const notify = () => { for (const callback of listeners) callback(); };
    const push = history.pushState; const replace = history.replaceState;
    const wrappedPush: History['pushState'] = function (this: History, ...args) { push.apply(this, args); notify(); };
    const wrappedReplace: History['replaceState'] = function (this: History, ...args) { replace.apply(this, args); notify(); };
    history.pushState = wrappedPush; history.replaceState = wrappedReplace;
    window.addEventListener('popstate', notify); window.addEventListener('hashchange', notify);
    hub = {listeners, stop: () => {
      if (history.pushState === wrappedPush) history.pushState = push;
      if (history.replaceState === wrappedReplace) history.replaceState = replace;
      window.removeEventListener('popstate', notify); window.removeEventListener('hashchange', notify);
      delete registry[hubKey];
    }};
    registry[hubKey] = hub;
  }
  hub.listeners.add(listener);
  const current = hub;
  return () => { current.listeners.delete(listener); if (!current.listeners.size) current.stop(); };
}
