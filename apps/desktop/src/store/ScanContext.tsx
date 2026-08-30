import { createContext, useCallback, useContext, useEffect, useState, ReactNode } from "react";
import { api, DashboardData } from "../api";

interface ScanContextValue {
  dashboard: DashboardData | null;
  loading: boolean;
  scanning: boolean;
  error: string | null;
  runScan: (root: string) => Promise<void>;
  refresh: () => Promise<void>;
}

const ScanContext = createContext<ScanContextValue | null>(null);

export function ScanProvider({ children }: { children: ReactNode }) {
  const [dashboard, setDashboard] = useState<DashboardData | null>(null);
  const [loading, setLoading] = useState(true);
  const [scanning, setScanning] = useState(false);
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
      setScanning(true);
      setError(null);
      try {
        await api.runScan(root);
        await refresh();
      } catch (e) {
        setError(String(e));
      } finally {
        setScanning(false);
      }
    },
    [refresh],
  );

  useEffect(() => {
    refresh();
  }, [refresh]);

  return (
    <ScanContext.Provider value={{ dashboard, loading, scanning, error, runScan, refresh }}>
      {children}
    </ScanContext.Provider>
  );
}

export function useScan(): ScanContextValue {
  const ctx = useContext(ScanContext);
  if (!ctx) throw new Error("useScan must be used within a ScanProvider");
  return ctx;
}
