// Real (not Rust-internal) time-to-first-useful-result tracking. The clock
// starts when the frontend actually receives the scan-status="scanning"
// event (SCAN_STARTED, per the pipeline: Rust scanner -> persistence ->
// Tauri event -> frontend state), and each milestone reports the elapsed
// time back to the Rust terminal via report_frontend_timing so it lands in
// the same log as every other measurement, not just a devtools console.

import { invoke } from "@tauri-apps/api/core";

let scanStartTime: number | null = null;
const reportedThisScan = new Set<string>();

export function markScanStarted() {
  scanStartTime = performance.now();
  reportedThisScan.clear();
}

export function reportMilestone(label: string) {
  if (scanStartTime === null || reportedThisScan.has(label)) return;
  reportedThisScan.add(label);
  const elapsedMs = performance.now() - scanStartTime;
  invoke("report_frontend_timing", { label, elapsedMs }).catch(() => {});
}
