import { useEffect, useState } from "react";
import { useScan } from "../store/ScanContext";
import { api, formatBytes, AppAssociation } from "../api";

export default function Applications() {
  const { dashboard } = useScan();
  const [associations, setAssociations] = useState<AppAssociation[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!dashboard) return;
    setLoading(true);
    api
      .getAppAssociations(dashboard.scan_id)
      .then((data) => setAssociations(data.sort((a, b) => b.associated_size - a.associated_size)))
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [dashboard]);

  if (!dashboard) {
    return (
      <div>
        <h1 className="page-title">Applications</h1>
        <div className="empty-state">Scan a folder from Overview first.</div>
      </div>
    );
  }

  return (
    <div>
      <h1 className="page-title">Applications</h1>
      <p className="page-subtitle">Installed applications matched with their supporting storage in this scan.</p>

      {error && <div className="error-banner">{error}</div>}
      {loading && <div className="empty-state">Loading...</div>}
      {!loading && associations.length === 0 && (
        <div className="empty-state">No installed applications matched storage from this scan.</div>
      )}

      {!loading &&
        associations.map((a) => (
          <div key={a.app.name} className="card" style={{ marginBottom: 10 }}>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
              <div>
                <div style={{ fontWeight: 600 }}>
                  {a.app.name}{" "}
                  <span className={a.confidence >= 0.9 ? "badge badge-safe" : "badge badge-review"}>
                    {a.confidence >= 0.9 ? "High confidence" : "Medium confidence"}
                  </span>
                </div>
                {a.app.publisher && <div style={{ color: "var(--text-secondary)", fontSize: 12.5 }}>{a.app.publisher}</div>}
              </div>
              <div style={{ fontWeight: 600 }}>{formatBytes(a.associated_size)}</div>
            </div>
            <div style={{ marginTop: 8 }}>
              {a.associated_paths.map((p) => (
                <div key={p} style={{ fontSize: 12.5, color: "var(--text-secondary)", padding: "2px 0" }}>
                  {p}
                </div>
              ))}
            </div>
          </div>
        ))}
    </div>
  );
}
