import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import { useScan } from "../store/ScanContext";
import { formatBytes } from "../api";
import { CATEGORY_LABELS } from "../categories";

const ACTIVE = new Set(["starting", "scanning", "cancelling"]);

function formatElapsed(ms: number): string {
  const totalSeconds = Math.floor(ms / 1000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return minutes > 0 ? `${minutes}m ${seconds}s` : `${seconds}s`;
}

export default function Overview() {
  const { dashboard, loading, status, progress, error, runScan, cancelScan } = useScan();
  const [lastResult, setLastResult] = useState<string | null>(null);
  const isActive = ACTIVE.has(status);

  async function pickAndScan() {
    const selected = await open({ directory: true, multiple: false, title: "Choose a folder to scan" });
    if (typeof selected === "string") {
      setLastResult(null);
      await runScan(selected);
      setLastResult(status);
    }
  }

  return (
    <div>
      <h1 className="page-title">Overview</h1>
      <p className="page-subtitle">Know what is using your space, and what is actually safe to remove.</p>

      {error && <div className="error-banner">{error}</div>}

      <div className="card" style={{ marginBottom: 24 }}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
          <div>
            <div style={{ fontWeight: 600, marginBottom: 4 }}>
              {dashboard ? dashboard.root : "No scan yet"}
            </div>
            <div style={{ color: "var(--text-secondary)", fontSize: 13 }}>
              {dashboard?.scanned_at
                ? `Last scanned ${new Date(dashboard.scanned_at).toLocaleString()}`
                : "Choose a folder to see what is using your space."}
            </div>
          </div>
          {isActive ? (
            <button onClick={cancelScan} disabled={status === "cancelling"}>
              {status === "cancelling" ? "Cancelling..." : "Cancel Scan"}
            </button>
          ) : (
            <button className="primary" onClick={pickAndScan}>
              Scan a Folder
            </button>
          )}
        </div>

        {isActive && (
          <div style={{ marginTop: 16, borderTop: "1px solid var(--border)", paddingTop: 14 }}>
            <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 8 }}>
              <span className="spinner" aria-hidden />
              <strong>{status === "cancelling" ? "Cancelling..." : "Scanning..."}</strong>
              {progress && <span style={{ color: "var(--text-secondary)", fontSize: 12.5 }}>{formatElapsed(progress.elapsed_ms)} elapsed</span>}
            </div>
            {progress ? (
              <>
                <div style={{ fontSize: 13, lineHeight: 1.8 }}>
                  <strong>{progress.files_scanned.toLocaleString()}</strong> files, {" "}
                  <strong>{progress.dirs_scanned.toLocaleString()}</strong> folders, {" "}
                  <strong>{formatBytes(progress.total_logical_size)}</strong> analysed
                  {progress.skipped_total > 0 && (
                    <span style={{ color: "var(--review)" }}> ({progress.skipped_total.toLocaleString()} skipped)</span>
                  )}
                </div>
                {progress.current_path && (
                  <div style={{ fontSize: 12, color: "var(--text-secondary)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                    Current: {progress.current_path}
                  </div>
                )}
                {progress.elapsed_ms > 1000 && (
                  <div style={{ fontSize: 11.5, color: "var(--text-secondary)", marginTop: 4 }}>
                    {Math.round(progress.files_per_sec).toLocaleString()} files/sec -- {progress.mb_per_sec.toFixed(1)} MB/sec
                  </div>
                )}
              </>
            ) : (
              <div style={{ fontSize: 13, color: "var(--text-secondary)" }}>Starting...</div>
            )}
          </div>
        )}

        {!isActive && lastResult === "cancelled" && (
          <div style={{ marginTop: 12, fontSize: 12.5, color: "var(--review)" }}>
            Scan cancelled -- results below are partial, from what was scanned before you cancelled.
          </div>
        )}
      </div>

      {loading && <div className="empty-state">Loading...</div>}

      {!loading && dashboard && (
        <>
          <div className="stat-grid">
            <div className="card">
              <div className="stat-value">{formatBytes(dashboard.total_size)}</div>
              <div className="stat-label">Total scanned</div>
            </div>
            <div className="card">
              <div className="stat-value">{dashboard.category_totals.length}</div>
              <div className="stat-label">Recognised categories</div>
            </div>
            <div className="card">
              <div className="stat-value">
                {formatBytes(dashboard.category_totals.reduce((sum, c) => sum + c.total_size, 0))}
              </div>
              <div className="stat-label">Potentially reclaimable</div>
            </div>
          </div>

          <div className="card">
            <table>
              <thead>
                <tr>
                  <th>Category</th>
                  <th>Items</th>
                  <th>Size</th>
                </tr>
              </thead>
              <tbody>
                {dashboard.category_totals.length === 0 && (
                  <tr>
                    <td colSpan={3} className="empty-state">
                      {isActive ? "Analysing... results will appear here shortly." : "Nothing recognised yet in this scan."}
                    </td>
                  </tr>
                )}
                {dashboard.category_totals.map((c) => (
                  <tr key={c.category_id}>
                    <td>{CATEGORY_LABELS[c.category_id] ?? c.category_id}</td>
                    <td>{c.item_count}</td>
                    <td>{formatBytes(c.total_size)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}

      {!loading && !dashboard && !isActive && (
        <div className="empty-state">Scan a folder to see what is filling your storage.</div>
      )}
    </div>
  );
}
