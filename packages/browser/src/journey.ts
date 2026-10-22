import type {AnalyticsClient} from './types.js';

/** A capability travels in the fragment, never in referrers or HTTP request logs. */
export function takeHandoff(location: Location = window.location, history: History = window.history): string | null {
  const fragment = new URLSearchParams(location.hash.slice(1));
  const values = fragment.getAll('analytics_handoff');
  if (!values.length) return null;
  fragment.delete('analytics_handoff');
  history.replaceState(history.state, '', location.pathname + location.search + (fragment.size ? '#' + fragment.toString() : ''));
  return values.length === 1 && /^[A-Za-z0-9._~-]{16,2048}$/.test(values[0]!) ? values[0]! : null;
}

/** Analytics can delay a normal navigation only by this bounded amount. */
export async function handoffURL(client: AnalyticsClient, destination: string, timeoutMs = 1200): Promise<string> {
  const url = new URL(destination, location.href);
  if (url.hash) return url.href;
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      client.createHandoff(url.href).then(reference => {
        url.hash = new URLSearchParams({analytics_handoff: reference}).toString();
        return url.href;
      }),
      new Promise<string>(resolve => { timer = setTimeout(() => resolve(destination), timeoutMs); })
    ]);
  } catch { return destination; }
  finally { clearTimeout(timer); }
}

/** Normal same-tab links only. Modified/new-tab/download links keep native semantics. */
export function linkJourneys(client: AnalyticsClient, origins: readonly string[]): () => void {
  const allowed = new Set(origins);
  const click = (event: MouseEvent) => {
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const elements = event.composedPath().filter((item): item is Element => item instanceof Element);
    if (elements.some(item => item.hasAttribute('data-analytics-ignore'))) return;
    const link = elements.find(item => item instanceof HTMLAnchorElement) as HTMLAnchorElement | undefined;
    if (!link || link.download || (link.target && link.target !== '_self')) return;
    const url = new URL(link.href);
    if (!allowed.has(url.origin) || url.origin === location.origin || url.hash) return;
    event.preventDefault();
    void handoffURL(client, url.href).then(target => location.assign(target));
  };
  document.addEventListener('click', click);
  return () => document.removeEventListener('click', click);
}
