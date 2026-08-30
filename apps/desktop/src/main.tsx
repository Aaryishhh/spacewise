import React from "react";
import ReactDOM from "react-dom/client";
import { HashRouter } from "react-router-dom";
import App from "./App";
import { ScanProvider } from "./store/ScanContext";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <HashRouter>
      <ScanProvider>
        <App />
      </ScanProvider>
    </HashRouter>
  </React.StrictMode>,
);
