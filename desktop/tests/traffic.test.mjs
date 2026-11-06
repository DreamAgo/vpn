import test from "node:test";
import assert from "node:assert/strict";
import { sampleTraffic } from "../src/traffic.ts";
import { formatRate } from "../src/format.ts";

const snapshot = (rx, tx, overrides = {}) => ({
  state: "connected", vpn_ip: "192.168.133.3", since: 1000,
  bytes_rx: rx, bytes_tx: tx, ...overrides,
});
const first = (status = snapshot(100, 50), at = 1000) => sampleTraffic(null, status, at).sample;

test("download and upload use independent counter deltas and actual elapsed time", () => {
  const { rates } = sampleTraffic(first(), snapshot(3100, 1550), 2500);
  assert.deepEqual(rates, { rx: 2000, tx: 1000 });
});
test("idle traffic returns zero rather than retaining the previous speed", () => {
  const previous = first(snapshot(1234, 5678));
  assert.deepEqual(sampleTraffic(previous, snapshot(1234, 5678), 2000).rates, { rx: 0, tx: 0 });
});
test("first reading and new sessions establish a baseline instead of a lifetime average", () => {
  assert.equal(sampleTraffic(null, snapshot(1e9, 2e9), 1000).rates, null);
  for (const change of [{ since: 1001 }, { vpn_ip: "192.168.133.4" }]) {
    assert.equal(sampleTraffic(first(), snapshot(1e9, 2e9, change), 2000).rates, null);
  }
});
test("counter resets rebase both directions and resume on the next fresh sample", () => {
  const reset = sampleTraffic(first(snapshot(10000, 20000)), snapshot(1, 2), 2000);
  assert.equal(reset.rates, null);
  assert.deepEqual(sampleTraffic(reset.sample, snapshot(1025, 514), 3000).rates, { rx: 1024, tx: 512 });
});
test("disconnect, reconnect and errors clear the sampling baseline", () => {
  for (const state of ["disconnected", "reconnecting", "connecting", "error"]) {
    assert.deepEqual(sampleTraffic(first(), snapshot(1000, 1000, { state }), 2000), { sample: null, rates: null });
  }
});
test("long background gaps and non-increasing sample clocks never produce a speed spike", () => {
  for (const at of [1000, 999, 6001]) {
    assert.equal(sampleTraffic(first(), snapshot(1e9, 2e9), at).rates, null);
  }
});
test("invalid samples are unavailable and cannot contaminate the next measurement", () => {
  for (const value of [NaN, Infinity, -1]) {
    assert.equal(sampleTraffic(first(), snapshot(value, 50), 2000).sample, null);
    assert.equal(sampleTraffic(first(), snapshot(100, value), 2000).sample, null);
  }
  assert.equal(sampleTraffic(first(), snapshot(100, 50), NaN).sample, null);
});
test("sampling keeps its own copy of the counters", () => {
  const status = snapshot(100, 50);
  const previous = first(status);
  status.bytes_rx = 999999;
  assert.deepEqual(sampleTraffic(previous, snapshot(1100, 550), 2000).rates, { rx: 1000, tx: 500 });
});
test("rates distinguish unavailable values from zero and scale byte units", () => {
  assert.equal(formatRate(null), "--");
  assert.equal(formatRate(NaN), "--");
  assert.equal(formatRate(-1), "--");
  assert.equal(formatRate(0), "0 B/s");
  assert.equal(formatRate(512), "512 B/s");
  assert.equal(formatRate(1536), "1.5 KB/s");
  assert.equal(formatRate(1048576), "1.0 MB/s");
});
