import { open } from "@tauri-apps/plugin-dialog";
import { useScan } from "../store/ScanContext";
import { formatBytes } from "../api";
import { CATEGORY_LABELS } from "../categories";

export default function Overview() {
  const { dashboard, loading, scanning, error, runScan } = useScan();

  async function pickAndScan() {
    const selected = await open({ directory: true, multiple: false, title: "Choose a folder to scan" });
    if (typeof selected === "string") {
      await runScan(selected);
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
              {dashboard?.scanned_at ? `Last scanned ${new Date(dashboard.scanned_at).toLocaleString()}` : "Choose a folder to see what is using your space."}
            </div>
          </div>
          <button className="primary" onClick={pickAndScan} disabled={scanning}>
            {scanning ? "Scanning..." : "Scan a Folder"}
          </button>
        </div>
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
                      Nothing recognised yet in this scan.
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

      {!loading && !dashboard && (
        <div className="empty-state">Scan a folder to see what is filling your storage.</div>
      )}
    </div>
  );
}
