import { createContext, useCallback, useContext, useEffect, useRef, useState, ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, DashboardData } from "../api";

export interface ScanProgress {
  files_scanned: number;
  dirs_scanned: number;
  total_logical_size: number;
}

interface ScanContextValue {
  dashboard: DashboardData | null;
  loading: boolean;
  scanning: boolean;
  progress: ScanProgress | null;
  error: string | null;
  runScan: (root: string) => Promise<void>;
  refresh: () => Promise<void>;
}

const ScanContext = createContext<ScanContextValue | null>(null);

export function ScanProvider({ children }: { children: ReactNode }) {
  const [dashboard, setDashboard] = useState<DashboardData | null>(null);
  const [loading, setLoading] = useState(true);
  const [scanning, setScanning] = useState(false);
  const [progress, setProgress] = useState<ScanProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const scanningRef = useRef(scanning);
  scanningRef.current = scanning;

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
      setScanning(true);
      setProgress(null);
      setError(null);
      try {
        await api.runScan(root);
        await refresh();
      } catch (e) {
        setError(String(e));
      } finally {
        setScanning(false);
        setProgress(null);
      }
    },
    [refresh],
  );

  // Live progress while a scan is running -- without this, a large scan
  // (e.g. a whole drive) gives zero feedback for minutes and looks hung
  // even though it is working; the backend now streams these events from a
  // spawn_blocking task so the window itself never freezes either.
  useEffect(() => {
    const unlistenProgress = listen<ScanProgress>("scan-progress", (event) => {
      if (scanningRef.current) setProgress(event.payload);
    });
    return () => {
      unlistenProgress.then((f) => f());
    };
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  return (
    <ScanContext.Provider value={{ dashboard, loading, scanning, progress, error, runScan, refresh }}>
      {children}
    </ScanContext.Provider>
  );
}

export function useScan(): ScanContextValue {
  const ctx = useContext(ScanContext);
  if (!ctx) throw new Error("useScan must be used within a ScanProvider");
  return ctx;
}
