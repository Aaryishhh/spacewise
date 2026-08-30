import React from "react";
import ReactDOM from "react-dom/client";
import { HashRouter } from "react-router-dom";
import App from "./App";
import { ScanProvider } from "./store/ScanContext";
import { ManualBasketProvider } from "./store/ManualBasketContext";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <HashRouter>
      <ScanProvider>
        <ManualBasketProvider>
          <App />
        </ManualBasketProvider>
      </ScanProvider>
    </HashRouter>
  </React.StrictMode>,
);
