import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

interface CoreStatus {
  version: string;
  phase: string;
}

function App() {
  const [status, setStatus] = useState<CoreStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<CoreStatus>("core_status")
      .then(setStatus)
      .catch((e) => setError(String(e)));
  }, []);

  return (
    <main className="container">
      <h1>Spacewise</h1>
      <p>Know what's using your space — and what's actually safe to remove.</p>
      {error && <p className="error">core bridge error: {error}</p>}
      {status && (
        <p>
          spacewise-core v{status.version} — {status.phase}
        </p>
      )}
    </main>
  );
}

export default App;
