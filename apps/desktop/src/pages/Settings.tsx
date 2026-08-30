export default function Settings() {
  return (
    <div>
      <h1 className="page-title">Settings</h1>
      <p className="page-subtitle">Spacewise scans and stores everything locally. Nothing leaves this computer.</p>

      <div className="card">
        <div style={{ fontWeight: 600, marginBottom: 6 }}>Privacy</div>
        <div style={{ color: "var(--text-secondary)", fontSize: 13.5, lineHeight: 1.6 }}>
          All scanning happens on this device. No file names, paths, or contents are ever sent anywhere.
          There is no telemetry, no account, and no network access required for any feature.
        </div>
      </div>
    </div>
  );
}
