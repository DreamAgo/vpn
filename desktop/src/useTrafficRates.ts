import { useEffect, useState } from "react";
import { getStatus } from "./api";
import { sampleTraffic, type TrafficRates, type TrafficSample } from "./traffic";

// A lightweight status-only poll; account/configuration refresh stays unchanged.
export function useTrafficRates(active: boolean, sessionKey: string): TrafficRates | null {
  const [reading, setReading] = useState<{ key: string; rates: TrafficRates | null } | null>(null);
  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;
    let staleTimer: number | undefined;
    let previous: TrafficSample | null = null;
    setReading(null);
    if (!active) return;

    const poll = async () => {
      staleTimer = window.setTimeout(() => {
        if (disposed) return;
        previous = null;
        setReading(null);
      }, 3000);
      try {
        const status = await getStatus();
        if (disposed) return;
        const next = sampleTraffic(previous, status, performance.now());
        previous = next.sample;
        setReading({ key: sessionKey, rates: next.rates });
      } catch {
        if (disposed) return;
        previous = null;
        setReading(null);
      } finally {
        window.clearTimeout(staleTimer);
        // Schedule after completion: slow backend calls never overlap or race.
        if (!disposed) timer = window.setTimeout(poll, 1000);
      }
    };
    void poll();
    return () => {
      disposed = true;
      if (timer !== undefined) window.clearTimeout(timer);
      if (staleTimer !== undefined) window.clearTimeout(staleTimer);
    };
  }, [active, sessionKey]);

  return active && reading?.key === sessionKey ? reading.rates : null;
}
