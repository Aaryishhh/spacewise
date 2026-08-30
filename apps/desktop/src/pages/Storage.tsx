import { useCallback, useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useScan } from "../store/ScanContext";
import { useManualBasket } from "../store/ManualBasketContext";
import { api, formatBytes, TreemapChild, TreemapNode } from "../api";
import Treemap from "../components/Treemap";
import ContextMenu, { ContextMenuItem } from "../components/ContextMenu";
import { reportMilestone } from "../lib/perf";

type SortKey = "size" | "name" | "modified" | "items";

function splitPath(path: string): string[] {
  return path.split(/[\\/]/).filter(Boolean);
}

export default function Storage() {
  const { dashboard, status } = useScan();
  const basket = useManualBasket();
  const [path, setPath] = useState<string | null>(null);
  const [node, setNode] = useState<TreemapNode | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [sortKey, setSortKey] = useState<SortKey>("size");
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number; child: TreemapChild } | null>(null);

  useEffect(() => {
    if (dashboard && !path) setPath(dashboard.root);
  }, [dashboard, path]);

  const load = useCallback(async () => {
    if (!dashboard || !path) return;
    try {
      const data = await api.getTreemapNode(dashboard.scan_id, path);
      reportMilestone("FIRST_TREEMAP_DATA_AVAILABLE"); // backend responded with a node, before paint
      setNode(data);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [dashboard, path]);

  // FIRST_TREEMAP_RENDERED: fires after React has actually committed the
  // treemap with non-empty children to the DOM (queued via
  // requestAnimationFrame so it reflects the paint, not just the state
  // update that triggers it).
  useEffect(() => {
    if (node && node.children.length > 0) {
      requestAnimationFrame(() => reportMilestone("FIRST_TREEMAP_RENDERED"));
    }
  }, [node]);

  useEffect(() => {
    setLoading(true);
    load();
  }, [load]);

  // Progressive treemap: re-fetch the current node whenever the backend
  // persists a new partial aggregate snapshot (throttled server-side to
  // ~1.5s) so the view updates a few times per second at most, never
  // reshuffling on every filesystem event.
  useEffect(() => {
    const unlisten = listen("dashboard-updated", () => {
      if (status === "scanning" || status === "starting") load();
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [load, status]);

  const drill = useCallback((child: TreemapChild) => {
    if (child.type !== "directory") return;
    setPath(child.path);
    setSelectedPath(null);
  }, []);

  const sortedChildren = useMemo(() => {
    if (!node) return [];
    const items = [...node.children];
    items.sort((a, b) => {
      switch (sortKey) {
        case "name":
          return a.name.localeCompare(b.name);
        case "modified":
          return (b.modified_at ?? "").localeCompare(a.modified_at ?? "");
        case "items":
          return b.child_count - a.child_count;
        case "size":
        default:
          return b.size - a.size;
      }
    });
    return items;
  }, [node, sortKey]);

  if (!dashboard) {
    return (
      <div>
        <h1 className="page-title">Storage</h1>
        <div className="empty-state">Scan a folder from Overview first.</div>
      </div>
    );
  }

  const crumbs = path ? splitPath(path) : [];
  const rootCrumbCount = path ? splitPath(dashboard.root).length : 0;

  function contextMenuItems(child: TreemapChild): ContextMenuItem[] {
    const items: ContextMenuItem[] = [];
    if (child.type !== "other") {
      items.push({ label: "Reveal in File Manager", onClick: () => api.revealInFileManager(child.path) });
    }
    if (child.type === "directory") {
      items.push({ label: "Open (Zoom In)", onClick: () => drill(child) });
    }
    items.push({
      label: "Copy Path",
      onClick: () => navigator.clipboard?.writeText(child.path),
    });
    if (child.type !== "other") {
      items.push({
        label: "Add to Cleanup",
        onClick: () =>
          basket.add({
            category_id: "manual",
            display_name: child.name,
            paths: [child.path],
            total_size: child.size,
            safety: "Review",
          }),
      });
    }
    return items;
  }

  return (
    <div>
      <h1 className="page-title">Storage</h1>
      <p className="page-subtitle">Drill into {dashboard.root} to see what is using space.</p>

      {error && <div className="error-banner">{error}</div>}

      <div className="breadcrumbs">
        {crumbs.map((part, i) => {
          const targetPath = i === 0 ? crumbs[0] + (part.endsWith(":") ? "\\" : "") : crumbs.slice(0, i + 1).join("\\");
          const isLast = i === crumbs.length - 1;
          const isBelowRoot = i < rootCrumbCount - 1;
          if (isBelowRoot) return null;
          return (
            <span key={i}>
              {i > rootCrumbCount - 1 && " > "}
              {isLast ? <strong>{part}</strong> : <button onClick={() => setPath(targetPath)}>{part}</button>}
            </span>
          );
        })}
      </div>

      <Treemap
        children={node?.children ?? []}
        selectedPath={selectedPath}
        onSelect={(child) => setSelectedPath(child.path)}
        onDrill={drill}
        onContextMenu={(child, x, y) => {
          setSelectedPath(child.path);
          setContextMenu({ x, y, child });
        }}
      />
      {loading && <div style={{ fontSize: 12, color: "var(--text-secondary)", marginTop: 6 }}>Updating...</div>}

      {contextMenu && (
        <ContextMenu x={contextMenu.x} y={contextMenu.y} items={contextMenuItems(contextMenu.child)} onClose={() => setContextMenu(null)} />
      )}

      <div className="card" style={{ marginTop: 20 }}>
        <div style={{ display: "flex", gap: 8, marginBottom: 10 }}>
          {(["size", "name", "modified", "items"] as SortKey[]).map((key) => (
            <button key={key} className={sortKey === key ? "primary" : ""} onClick={() => setSortKey(key)}>
              Sort by {key}
            </button>
          ))}
        </div>
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th>Size</th>
              <th>% of parent</th>
              <th>Items</th>
              <th>Modified</th>
            </tr>
          </thead>
          <tbody>
            {sortedChildren.length === 0 && (
              <tr>
                <td colSpan={5} className="empty-state">
                  Nothing here yet.
                </td>
              </tr>
            )}
            {sortedChildren.map((c) => (
              <tr
                key={c.path}
                onClick={() => setSelectedPath(c.path)}
                onDoubleClick={() => drill(c)}
                style={{ cursor: "pointer", background: selectedPath === c.path ? "var(--hover-bg)" : undefined }}
              >
                <td>{c.name}</td>
                <td>{formatBytes(c.size)}</td>
                <td>{node && node.total_size > 0 ? ((c.size / node.total_size) * 100).toFixed(1) : "0"}%</td>
                <td>{c.type === "file" ? "--" : c.child_count}</td>
                <td>{c.modified_at ? new Date(c.modified_at).toLocaleDateString() : "--"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
