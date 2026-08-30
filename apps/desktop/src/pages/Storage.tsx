import { useEffect, useState } from "react";
import { useScan } from "../store/ScanContext";
import { api, formatBytes, DirectoryAggregate } from "../api";

export default function Storage() {
  const { dashboard } = useScan();
  const [path, setPath] = useState<string | null>(null);
  const [children, setChildren] = useState<DirectoryAggregate[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (dashboard) setPath(dashboard.root);
  }, [dashboard]);

  useEffect(() => {
    if (!dashboard || !path) return;
    setLoading(true);
    api
      .getDirectoryChildren(dashboard.scan_id, path)
      .then(setChildren)
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [dashboard, path]);

  if (!dashboard) {
    return (
      <div>
        <h1 className="page-title">Storage</h1>
        <div className="empty-state">Scan a folder from Overview first.</div>
      </div>
    );
  }

  const parts = path?.split(/[\/]/).filter(Boolean) ?? [];
  const maxSize = Math.max(1, ...children.map((c) => c.total_size));

  return (
    <div>
      <h1 className="page-title">Storage</h1>
      <p className="page-subtitle">Drill down through {dashboard.root} to see what is using space.</p>

      {error && <div className="error-banner">{error}</div>}

      <div className="breadcrumbs">
        <button onClick={() => setPath(dashboard.root)}>{dashboard.root}</button>
        {path && path !== dashboard.root && <span> / {parts[parts.length - 1]}</span>}
      </div>

      <div className="card">
        {loading && <div className="empty-state">Loading...</div>}
        {!loading && children.length === 0 && <div className="empty-state">No subdirectories here.</div>}
        {!loading && children.length > 0 && (
          <table>
            <thead>
              <tr>
                <th>Name</th>
                <th>Files</th>
                <th>Size</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {children.map((c) => {
                const name = c.path.split(/[\/]/).filter(Boolean).pop() ?? c.path;
                const pct = Math.round((c.total_size / maxSize) * 100);
                return (
                  <tr key={c.path}>
                    <td>
                      <button onClick={() => setPath(c.path)} style={{ border: "none", background: "none", padding: 0, color: "var(--text)", cursor: "pointer", textAlign: "left" }}>
                        {name}
                      </button>
                    </td>
                    <td>{c.file_count}</td>
                    <td style={{ minWidth: 220 }}>
                      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                        <div style={{ flex: 1, height: 6, background: "var(--hover-bg)", borderRadius: 3, overflow: "hidden" }}>
                          <div style={{ width: `${pct}%`, height: "100%", background: "var(--accent)" }} />
                        </div>
                        <span style={{ whiteSpace: "nowrap" }}>{formatBytes(c.total_size)}</span>
                      </div>
                    </td>
                    <td>
                      <button onClick={() => api.revealPath(c.path)}>Reveal</button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
