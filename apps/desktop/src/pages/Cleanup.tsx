import { useEffect, useState } from "react";
import { useScan } from "../store/ScanContext";
import { useManualBasket } from "../store/ManualBasketContext";
import { api, formatBytes, Recommendation, CleanupCandidate, CleanupOutcome } from "../api";
import { CATEGORY_LABELS } from "../categories";
import SafetyBadge from "../components/SafetyBadge";

export default function Cleanup() {
  const { dashboard, refresh } = useScan();
  const manualBasket = useManualBasket();
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

  // Manually-added items (e.g. "Add to Cleanup" from the treemap) start
  // selected -- the user already chose them explicitly once.
  useEffect(() => {
    setSelected((prev) => {
      const next = new Set(prev);
      for (const item of manualBasket.items) next.add(item.id);
      return next;
    });
  }, [manualBasket.items]);

  if (!dashboard) {
    return (
      <div>
        <h1 className="page-title">Cleanup</h1>
        <div className="empty-state">Scan a folder from Overview first.</div>
      </div>
    );
  }

  const scanId = dashboard.scan_id;
  const allCandidates: CleanupCandidate[] = [...recommendations.map((r) => r.candidate), ...manualBasket.items];

  function toggle(id: string) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  const basketItems = allCandidates.filter((c) => selected.has(c.id));
  const basketTotal = basketItems.reduce((sum, c) => sum + c.total_size, 0);

  async function cleanSelected() {
    setCleaning(true);
    setOutcome(null);
    try {
      let failed: string[] = [];
      for (const candidate of basketItems) {
        const result = await api.executeCleanup(candidate);
        failed = failed.concat(result.failed);
      }
      setOutcome({ succeeded: [], failed });
      setSelected(new Set());
      manualBasket.clear();
      await api.getRecommendations(scanId).then(setRecommendations);
      await refresh();
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

      {manualBasket.items.length > 0 && (
        <>
          <h2 style={{ fontSize: 15, fontWeight: 600, margin: "0 0 10px" }}>Manually Added</h2>
          {manualBasket.items.map((item) => (
            <div key={item.id} className="card" style={{ marginBottom: 10, display: "flex", alignItems: "center", gap: 14 }}>
              <input type="checkbox" checked={selected.has(item.id)} onChange={() => toggle(item.id)} />
              <div style={{ flex: 1 }}>
                <div style={{ fontWeight: 600 }}>
                  {item.display_name} <SafetyBadge level={item.safety} />
                </div>
                <div style={{ color: "var(--text-secondary)", fontSize: 12.5, marginTop: 2 }}>{item.paths[0]}</div>
              </div>
              <div style={{ fontWeight: 600 }}>{formatBytes(item.total_size)}</div>
              <button onClick={() => manualBasket.remove(item.id)}>Remove</button>
            </div>
          ))}
          <h2 style={{ fontSize: 15, fontWeight: 600, margin: "20px 0 10px" }}>Recommended</h2>
        </>
      )}

      {loading && <div className="empty-state">Loading recommendations...</div>}

      {!loading && recommendations.length === 0 && manualBasket.items.length === 0 && (
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
