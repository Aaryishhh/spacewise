import { useEffect, useState } from "react";
import { useScan } from "../store/ScanContext";
import { api, formatBytes, FileEntry } from "../api";

const SIZE_OPTIONS = [
  { label: "> 100 MB", bytes: 100 * 1024 * 1024 },
  { label: "> 500 MB", bytes: 500 * 1024 * 1024 },
  { label: "> 1 GB", bytes: 1024 * 1024 * 1024 },
  { label: "> 5 GB", bytes: 5 * 1024 * 1024 * 1024 },
];

export default function LargeFiles() {
  const { dashboard } = useScan();
  const [minSize, setMinSize] = useState(SIZE_OPTIONS[0].bytes);
  const [files, setFiles] = useState<FileEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!dashboard) return;
    setLoading(true);
    api
      .getLargeFiles(dashboard.scan_id, minSize)
      .then(setFiles)
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [dashboard, minSize]);

  if (!dashboard) {
    return (
      <div>
        <h1 className="page-title">Large Files</h1>
        <div className="empty-state">Scan a folder from Overview first.</div>
      </div>
    );
  }

  return (
    <div>
      <h1 className="page-title">Large Files</h1>
      <p className="page-subtitle">The biggest individual files in this scan.</p>

      {error && <div className="error-banner">{error}</div>}

      <div style={{ marginBottom: 16, display: "flex", gap: 8 }}>
        {SIZE_OPTIONS.map((opt) => (
          <button key={opt.bytes} className={minSize === opt.bytes ? "primary" : ""} onClick={() => setMinSize(opt.bytes)}>
            {opt.label}
          </button>
        ))}
      </div>

      <div className="card">
        {loading && <div className="empty-state">Loading...</div>}
        {!loading && files.length === 0 && <div className="empty-state">No files above this size.</div>}
        {!loading && files.length > 0 && (
          <table>
            <thead>
              <tr>
                <th>Path</th>
                <th>Modified</th>
                <th>Size</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {files.map((f) => (
                <tr key={f.id}>
                  <td style={{ maxWidth: 500, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }} title={f.path}>
                    {f.path}
                  </td>
                  <td>{f.modified_at ? new Date(f.modified_at).toLocaleDateString() : "--"}</td>
                  <td>{formatBytes(f.logical_size)}</td>
                  <td>
                    <button onClick={() => api.revealInFileManager(f.path)}>Reveal</button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
