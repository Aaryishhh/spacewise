import { createContext, useCallback, useContext, useEffect, useState, ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, DashboardData, ScanStatus, ScanProgressPayload } from "../api";
import { markScanStarted, reportMilestone } from "../lib/perf";

interface ScanContextValue {
  dashboard: DashboardData | null;
  loading: boolean;
  status: ScanStatus;
  progress: ScanProgressPayload | null;
  error: string | null;
  runScan: (root: string) => Promise<void>;
  cancelScan: () => Promise<void>;
  refresh: () => Promise<void>;
}

const ScanContext = createContext<ScanContextValue | null>(null);

const ACTIVE_STATUSES: ScanStatus[] = ["starting", "scanning", "cancelling"];

export function ScanProvider({ children }: { children: ReactNode }) {
  const [dashboard, setDashboard] = useState<DashboardData | null>(null);
  const [loading, setLoading] = useState(true);
  const [status, setStatus] = useState<ScanStatus>("idle");
  const [progress, setProgress] = useState<ScanProgressPayload | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const data = await api.getDashboard();
      setDashboard(data);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  const runScan = useCallback(
    async (root: string) => {
      setProgress(null);
      setError(null);
      try {
        await api.runScan(root);
        // Final refresh in case the last dashboard-updated event landed
        // before the very last batch was persisted.
        await refresh();
      } catch (e) {
        setError(String(e));
      }
    },
    [refresh],
  );

  const cancelScan = useCallback(async () => {
    try {
      await api.cancelScan();
    } catch (e) {
      setError(String(e));
    }
  }, []);

  // The scan-status event is the single source of truth for UI state (spec:
  // "UI state should be driven by this model rather than scattered
  // booleans") -- everything else (progress numbers, dashboard data) is
  // just detail hanging off whichever status we are currently in.
  useEffect(() => {
    const unlistenStatus = listen<{ status: ScanStatus }>("scan-status", (event) => {
      if (event.payload.status === "scanning") {
        markScanStarted(); // SCAN_STARTED, from the frontend's own clock
      }
      setStatus(event.payload.status);
      if (!ACTIVE_STATUSES.includes(event.payload.status)) {
        setProgress(null);
      }
    });
    const unlistenProgress = listen<ScanProgressPayload>("scan-progress", (event) => {
      setProgress(event.payload);
    });
    // Progressive results: as soon as the backend has persisted a partial
    // aggregate snapshot, re-fetch the dashboard so Overview/Storage show
    // real numbers within seconds instead of waiting for the whole scan.
    const unlistenDashboard = listen("dashboard-updated", () => {
      reportMilestone("FIRST_AGGREGATE_AVAILABLE"); // first partial-aggregate event reaching the frontend
      refresh();
    });
    return () => {
      unlistenStatus.then((f) => f());
      unlistenProgress.then((f) => f());
      unlistenDashboard.then((f) => f());
    };
  }, [refresh]);

  useEffect(() => {
    api.getScanStatus().then((s) => setStatus(s.status)).catch(() => {});
    refresh();
  }, [refresh]);

  return (
    <ScanContext.Provider value={{ dashboard, loading, status, progress, error, runScan, cancelScan, refresh }}>
      {children}
    </ScanContext.Provider>
  );
}

export function useScan(): ScanContextValue {
  const ctx = useContext(ScanContext);
  if (!ctx) throw new Error("useScan must be used within a ScanProvider");
  return ctx;
}
