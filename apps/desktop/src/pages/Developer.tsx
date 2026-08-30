import { useEffect, useState } from "react";
import { useScan } from "../store/ScanContext";
import { api, formatBytes, CategoryTotal } from "../api";
import { CATEGORY_LABELS } from "../categories";

export default function Developer() {
  const { dashboard } = useScan();
  const [totals, setTotals] = useState<CategoryTotal[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!dashboard) return;
    setLoading(true);
    api
      .getDeveloperStorage(dashboard.scan_id)
      .then(setTotals)
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [dashboard]);

  if (!dashboard) {
    return (
      <div>
        <h1 className="page-title">Developer</h1>
        <div className="empty-state">Scan a folder from Overview first.</div>
      </div>
    );
  }

  const total = totals.reduce((sum, t) => sum + t.total_size, 0);

  return (
    <div>
      <h1 className="page-title">Developer Storage</h1>
      <p className="page-subtitle">Build caches, package manager caches, and IDE data recognised in this scan.</p>

      {error && <div className="error-banner">{error}</div>}

      <div className="stat-grid">
        <div className="card">
          <div className="stat-value">{formatBytes(total)}</div>
          <div className="stat-label">Total developer storage</div>
        </div>
      </div>

      <div className="card">
        {loading && <div className="empty-state">Loading...</div>}
        {!loading && totals.length === 0 && <div className="empty-state">No developer tooling storage recognised yet.</div>}
        {!loading && totals.length > 0 && (
          <table>
            <thead>
              <tr>
                <th>Category</th>
                <th>Locations</th>
                <th>Size</th>
              </tr>
            </thead>
            <tbody>
              {totals.map((t) => (
                <tr key={t.category_id}>
                  <td>{CATEGORY_LABELS[t.category_id] ?? t.category_id}</td>
                  <td>{t.item_count}</td>
                  <td>{formatBytes(t.total_size)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
