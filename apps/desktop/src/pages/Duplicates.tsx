import { useEffect, useState } from "react";
import { useScan } from "../store/ScanContext";
import { api, formatBytes, DuplicateGroup } from "../api";

export default function Duplicates() {
  const { dashboard } = useScan();
  const [groups, setGroups] = useState<DuplicateGroup[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!dashboard) return;
    setLoading(true);
    api
      .getDuplicates(dashboard.scan_id)
      .then(setGroups)
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [dashboard]);

  if (!dashboard) {
    return (
      <div>
        <h1 className="page-title">Duplicates</h1>
        <div className="empty-state">Scan a folder from Overview first.</div>
      </div>
    );
  }

  const totalRecoverable = groups.reduce((sum, g) => sum + g.size * (g.paths.length - 1), 0);

  return (
    <div>
      <h1 className="page-title">Duplicate Files</h1>
      <p className="page-subtitle">
        Confirmed by content hash, not filename. {groups.length} group(s), {formatBytes(totalRecoverable)} recoverable if you keep one copy of each.
      </p>

      {error && <div className="error-banner">{error}</div>}
      {loading && <div className="empty-state">Loading...</div>}
      {!loading && groups.length === 0 && <div className="empty-state">No duplicate files found in this scan.</div>}

      {!loading &&
        groups.map((g) => (
          <div key={g.id} className="card" style={{ marginBottom: 10 }}>
            <div style={{ fontWeight: 600, marginBottom: 6 }}>
              {formatBytes(g.size)} each -- {g.paths.length} copies
            </div>
            {g.paths.map((p) => (
              <div key={p} style={{ display: "flex", justifyContent: "space-between", fontSize: 13, padding: "4px 0", color: "var(--text-secondary)" }}>
                <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{p}</span>
                <button onClick={() => api.revealInFileManager(p)}>Reveal</button>
              </div>
            ))}
          </div>
        ))}
    </div>
  );
}
