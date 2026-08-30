import { useEffect, useState } from "react";
import { api, formatBytes, GrowthSummary, CleanupAction } from "../api";
import { CATEGORY_LABELS } from "../categories";

export default function History() {
  const [summary, setSummary] = useState<GrowthSummary | null>(null);
  const [actions, setActions] = useState<CleanupAction[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [days, setDays] = useState(30);

  useEffect(() => {
    setLoading(true);
    api
      .getGrowthSummary(days)
      .then(setSummary)
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [days]);

  useEffect(() => {
    api.getCleanupHistory().then(setActions).catch((e) => setError(String(e)));
  }, []);

  return (
    <div>
      <h1 className="page-title">Why Is My Disk Full?</h1>
      <p className="page-subtitle">Comparing scans over the last {days} days.</p>

      {error && <div className="error-banner">{error}</div>}

      <div style={{ marginBottom: 16, display: "flex", gap: 8 }}>
        {[7, 30, 90].map((d) => (
          <button key={d} className={days === d ? "primary" : ""} onClick={() => setDays(d)}>
            Last {d} days
          </button>
        ))}
      </div>

      {loading && <div className="empty-state">Loading...</div>}

      {!loading && !summary && (
        <div className="empty-state">
          Not enough scan history yet. Run a scan again after some time has passed to see how your storage has changed.
        </div>
      )}

      {!loading && summary && (
        <>
          <div className="card" style={{ marginBottom: 16 }}>
            <div className="stat-value">
              {summary.growth_bytes >= 0 ? "+" : ""}
              {formatBytes(Math.abs(summary.growth_bytes))}
            </div>
            <div className="stat-label">
              {summary.growth_bytes >= 0 ? "Growth" : "Reduction"} over the last {summary.period_days} days
            </div>
          </div>

          <div className="card">
            <table>
              <thead>
                <tr>
                  <th>Category</th>
                  <th>Change</th>
                </tr>
              </thead>
              <tbody>
                {summary.top_contributors.map((c) => (
                  <tr key={c.category_id}>
                    <td>{CATEGORY_LABELS[c.category_id] ?? c.category_id}</td>
                    <td style={{ color: c.delta_bytes >= 0 ? "var(--never)" : "var(--safe)" }}>
                      {c.delta_bytes >= 0 ? "+" : ""}
                      {formatBytes(Math.abs(c.delta_bytes))}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}

      {actions.length > 0 && (
        <>
          <h2 style={{ fontSize: 15, fontWeight: 600, margin: "28px 0 12px" }}>Recent Cleanups</h2>
          <div className="card">
            <table>
              <thead>
                <tr>
                  <th>When</th>
                  <th>Category</th>
                  <th>Freed</th>
                  <th>Undoable</th>
                </tr>
              </thead>
              <tbody>
                {actions.map((a) => (
                  <tr key={a.id}>
                    <td>{new Date(a.performed_at).toLocaleString()}</td>
                    <td>{CATEGORY_LABELS[a.category_id] ?? a.category_id}</td>
                    <td>{formatBytes(a.bytes_freed)}</td>
                    <td>{a.undoable ? "Yes, via Trash/Recycle Bin" : "No"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}
    </div>
  );
}
