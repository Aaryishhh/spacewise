import { useEffect, useState } from "react";
import { useScan } from "../store/ScanContext";
import { api, formatBytes, Recommendation, CleanupOutcome } from "../api";
import { CATEGORY_LABELS } from "../categories";
import SafetyBadge from "../components/SafetyBadge";

export default function Cleanup() {
  const { dashboard, refresh } = useScan();
  const [recommendations, setRecommendations] = useState<Recommendation[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<CleanupOutcome | null>(null);
  const [cleaning, setCleaning] = useState(false);

  useEffect(() => {
    if (!dashboard) return;
    setLoading(true);
    api
      .getRecommendations(dashboard.scan_id)
      .then(setRecommendations)
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [dashboard]);

  if (!dashboard) {
    return (
      <div>
        <h1 className="page-title">Cleanup</h1>
        <div className="empty-state">Scan a folder from Overview first.</div>
      </div>
    );
  }

  const scanId = dashboard.scan_id;

  function toggle(id: string) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  const basketItems = recommendations.filter((r) => selected.has(r.candidate.id));
  const basketTotal = basketItems.reduce((sum, r) => sum + r.candidate.total_size, 0);

  async function cleanSelected() {
    setCleaning(true);
    setOutcome(null);
    try {
      let succeeded = 0;
      let failed: string[] = [];
      for (const rec of basketItems) {
        const result = await api.executeCleanup(rec.candidate);
        succeeded += result.succeeded.length;
        failed = failed.concat(result.failed);
      }
      setOutcome({ succeeded: [], failed });
      setSelected(new Set());
      await api.getRecommendations(scanId).then(setRecommendations);
      await refresh();
      if (failed.length === 0) {
        setOutcome({ succeeded: [], failed: [] });
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setCleaning(false);
    }
  }

  return (
    <div>
      <h1 className="page-title">Recommended Cleanup</h1>
      <p className="page-subtitle">Ranked by safety, regeneratability, and staleness -- not just size.</p>

      {error && <div className="error-banner">{error}</div>}
      {outcome && outcome.failed.length > 0 && (
        <div className="error-banner">Some items could not be cleaned: {outcome.failed.join("; ")}</div>
      )}
      {outcome && outcome.failed.length === 0 && <div className="card" style={{ marginBottom: 16, color: "var(--safe)" }}>Cleanup completed.</div>}

      {loading && <div className="empty-state">Loading recommendations...</div>}

      {!loading && recommendations.length === 0 && (
        <div className="empty-state">Nothing safe to recommend from this scan yet.</div>
      )}

      {!loading &&
        recommendations.map((rec) => (
          <div key={rec.candidate.id} className="card" style={{ marginBottom: 10, display: "flex", alignItems: "center", gap: 14 }}>
            <input type="checkbox" checked={selected.has(rec.candidate.id)} onChange={() => toggle(rec.candidate.id)} />
            <div style={{ flex: 1 }}>
              <div style={{ fontWeight: 600 }}>
                {CATEGORY_LABELS[rec.candidate.category_id] ?? rec.candidate.display_name}{" "}
                <SafetyBadge level={rec.candidate.safety} />
              </div>
              <div style={{ color: "var(--text-secondary)", fontSize: 12.5, marginTop: 2 }}>{rec.rationale}</div>
            </div>
            <div style={{ fontWeight: 600 }}>{formatBytes(rec.candidate.total_size)}</div>
          </div>
        ))}

      {basketItems.length > 0 && (
        <div className="card" style={{ marginTop: 20, display: "flex", justifyContent: "space-between", alignItems: "center", position: "sticky", bottom: 0 }}>
          <div>
            <strong>{basketItems.length}</strong> selected -- <strong>{formatBytes(basketTotal)}</strong> potential recovery
          </div>
          <button className="primary" onClick={cleanSelected} disabled={cleaning}>
            {cleaning ? "Cleaning..." : "Move to Trash"}
          </button>
        </div>
      )}
    </div>
  );
}
