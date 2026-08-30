import { Route, Routes } from "react-router-dom";
import Sidebar from "./components/Sidebar";
import Overview from "./pages/Overview";
import Storage from "./pages/Storage";
import Cleanup from "./pages/Cleanup";
import LargeFiles from "./pages/LargeFiles";
import Duplicates from "./pages/Duplicates";
import Applications from "./pages/Applications";
import Developer from "./pages/Developer";
import History from "./pages/History";
import Settings from "./pages/Settings";
import "./App.css";

function App() {
  return (
    <div className="app-shell">
      <Sidebar />
      <main className="app-content">
        <Routes>
          <Route path="/" element={<Overview />} />
          <Route path="/storage" element={<Storage />} />
          <Route path="/cleanup" element={<Cleanup />} />
          <Route path="/large-files" element={<LargeFiles />} />
          <Route path="/duplicates" element={<Duplicates />} />
          <Route path="/applications" element={<Applications />} />
          <Route path="/developer" element={<Developer />} />
          <Route path="/history" element={<History />} />
          <Route path="/settings" element={<Settings />} />
        </Routes>
      </main>
    </div>
  );
}

export default App;
