// Rates use monotonic sample times, never wall-clock connection duration.
export interface TrafficSnapshot {
  state: string;
  vpn_ip: string | null;
  since: number | null;
  bytes_rx: number;
  bytes_tx: number;
}

export interface TrafficRates {
  rx: number;
  tx: number;
}

export interface TrafficSample {
  status: TrafficSnapshot;
  at: number;
}

export function sampleTraffic(
  previous: TrafficSample | null,
  status: TrafficSnapshot,
  at: number,
): { sample: TrafficSample | null; rates: TrafficRates | null } {
  if (status.state !== "connected" || !Number.isFinite(at)
    || !Number.isFinite(status.bytes_rx) || status.bytes_rx < 0
    || !Number.isFinite(status.bytes_tx) || status.bytes_tx < 0) {
    return { sample: null, rates: null };
  }
  // Copy counters: later mutation of a caller's snapshot must not change a sample.
  const sample = { status: { ...status }, at };
  const elapsed = previous ? at - previous.at : 0;
  if (!previous || elapsed <= 0 || elapsed > 5000
    || status.since !== previous.status.since || status.vpn_ip !== previous.status.vpn_ip
    || status.bytes_rx < previous.status.bytes_rx || status.bytes_tx < previous.status.bytes_tx) {
    return { sample, rates: null };
  }
  return {
    sample,
    rates: {
      rx: (status.bytes_rx - previous.status.bytes_rx) * 1000 / elapsed,
      tx: (status.bytes_tx - previous.status.bytes_tx) * 1000 / elapsed,
    },
  };
}
