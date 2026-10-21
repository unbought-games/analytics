import type {Coverage, Observation, Options, Transport} from './types.js';
interface Entry {event: Observation; bytes: number; attempts: number}
export const encodedBytes = (value: unknown) => new TextEncoder().encode(JSON.stringify(value)).length;
export class DeliveryQueue {
  private entries: Entry[] = [];
  private active: Entry[] = [];
  private timer: ReturnType<typeof setTimeout> | undefined;
  private pending: Promise<void> | undefined;
  private controller: AbortController | undefined;
  private generation = 0;
  private stopped = false;
  readonly limits;
  constructor(private transport: Transport, private coverage: Coverage, options: Options['queue'], private onUndelivered: (event: Observation) => void = () => {}) {
    const limit = (n: number | undefined, fallback: number, max: number) => Math.max(1, Math.min(max, Math.floor(n ?? fallback)));
    this.limits = {
      maxEvents: limit(options?.maxEvents, 200, 1000), maxBytes: limit(options?.maxBytes, 131072, 1048576),
      batchSize: limit(options?.batchSize, 20, 100), batchBytes: limit(options?.batchBytes, 32768, 49152),
      interval: limit(options?.flushIntervalMs, 1000, 60000), maxAttempts: limit(options?.maxAttempts, 3, 5),
      timeout: limit(options?.requestTimeoutMs, 5000, 30000)
    };
  }
  push(event: Observation): boolean {
    const bytes = this.transport.bytes?.(event) ?? encodedBytes(event);
    if (bytes > Math.min(8192, this.limits.batchBytes - 256)) { this.coverage.dropped.oversized++; return false; }
    if (this.entries.length + this.active.length >= this.limits.maxEvents || this.coverage.queuedBytes + this.coverage.inFlightBytes + bytes > this.limits.maxBytes) {
      this.coverage.dropped.queue++; return false;
    }
    this.entries.push({event, bytes, attempts: 0});
    this.transport.enqueued?.(event);
    this.update(); this.schedule(); return true;
  }
  private update() {
    this.coverage.queuedEvents = this.entries.length;
    this.coverage.queuedBytes = this.entries.reduce((n, e) => n + e.bytes, 0);
    this.coverage.inFlightEvents = this.active.length;
    this.coverage.inFlightBytes = this.active.reduce((n, e) => n + e.bytes, 0);
    this.coverage.highWaterEvents = Math.max(this.coverage.highWaterEvents, this.entries.length + this.active.length);
    this.coverage.highWaterBytes = Math.max(this.coverage.highWaterBytes, this.coverage.queuedBytes + this.coverage.inFlightBytes);
  }
  private schedule() {
    if (!this.stopped && !this.timer && this.entries.length) this.timer = setTimeout(() => { this.timer = undefined; void this.flush(); }, this.limits.interval);
  }
  clear() {
    this.generation++;
    this.coverage.dropped.policy += this.entries.length + this.active.length;
    this.entries = []; this.active = [];
    this.controller?.abort();
    clearTimeout(this.timer); this.timer = undefined;
    this.update();
  }
  async shutdown() { this.stopped = true; clearTimeout(this.timer); this.timer = undefined; await this.flush(true); this.clear(); }
  flush(keepalive = false): Promise<void> {
    if (this.pending) return this.pending;
    clearTimeout(this.timer); this.timer = undefined;
    this.pending = this.drain(keepalive).finally(() => { this.pending = undefined; this.schedule(); });
    return this.pending;
  }
  private async drain(keepalive: boolean) {
    const generation = this.generation;
    let remaining = this.entries.length;
    const deadline = Date.now() + 30000;
    let batches = 0;
    while (remaining > 0 && this.entries.length && generation === this.generation && batches++ < 10 && Date.now() < deadline) {
      let bytes = 256;
      const batch: Entry[] = [];
      while (this.entries.length && batch.length < this.limits.batchSize && batch.length < remaining) {
        const entry = this.entries[0]!;
        if (bytes + entry.bytes > this.limits.batchBytes) break;
        batch.push(this.entries.shift()!); bytes += entry.bytes;
      }
      if (!batch.length) break;
      remaining -= batch.length;
      this.active = batch; this.update();
      this.controller = new AbortController();
      const timeout = setTimeout(() => this.controller?.abort(), Math.min(this.limits.timeout, Math.max(1, deadline - Date.now())));
      this.coverage.requests++; this.coverage.requestBytes += 12 + batch.reduce((sum, entry) => sum + entry.bytes + 1, 0);
      try {
        const result = await this.transport.send(batch.map(e => e.event), this.controller.signal, keepalive);
        if (generation !== this.generation) return;
        const accepted = new Set(result.accepted);
        const rejected = new Map(result.rejected.map(r => [r.id, r.retryable]));
        for (const entry of batch) {
          if (accepted.has(entry.event.id)) this.coverage.accepted++;
          else if (rejected.get(entry.event.id) === false) { this.coverage.rejected++; this.onUndelivered(entry.event); }
          else this.retry(entry);
        }
      } catch {
        if (generation !== this.generation) return;
        for (const entry of batch) this.retry(entry);
      } finally {
        clearTimeout(timeout);
        if (generation === this.generation) { this.active = []; this.update(); }
      }
      // An unavailable endpoint never spins within a single flush.
      if (batch.some(e => e.attempts > 0)) break;
    }
  }
  private retry(entry: Entry) {
    entry.attempts++;
    if (entry.attempts >= this.limits.maxAttempts) { this.coverage.dropped.exhausted++; this.onUndelivered(entry.event); }
    else this.entries.push(entry);
  }
}
