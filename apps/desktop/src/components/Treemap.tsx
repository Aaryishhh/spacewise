import { useMemo, useState } from "react";
import { TreemapChild, formatBytes } from "../api";
import { squarify } from "../lib/squarify";
import { categoryFor, colorFor } from "../lib/categoryColors";
import "./Treemap.css";

interface TreemapProps {
  children: TreemapChild[];
  selectedPath: string | null;
  onSelect: (child: TreemapChild) => void;
  onDrill: (child: TreemapChild) => void;
  onContextMenu: (child: TreemapChild, clientX: number, clientY: number) => void;
}

export default function Treemap({ children, selectedPath, onSelect, onDrill, onContextMenu }: TreemapProps) {
  const [hovered, setHovered] = useState<TreemapChild | null>(null);

  const rects = useMemo(
    () => squarify(children.map((c) => ({ id: c.path, size: c.size }))),
    [children],
  );
  const byPath = useMemo(() => new Map(children.map((c) => [c.path, c])), [children]);
  const totalSize = useMemo(() => children.reduce((s, c) => s + c.size, 0), [children]);

  if (children.length === 0) {
    return <div className="treemap empty-state">Nothing here yet.</div>;
  }

  return (
    <div className="treemap">
      {rects.map((rect) => {
        const child = byPath.get(rect.id);
        if (!child) return null;
        const category = categoryFor(child.name, child.type);
        const isSelected = selectedPath === child.path;
        const wide = rect.width > 8 && rect.height > 6;

        return (
          <div
            key={rect.id}
            className={`treemap-cell${isSelected ? " selected" : ""}`}
            style={{
              left: `${rect.x}%`,
              top: `${rect.y}%`,
              width: `${rect.width}%`,
              height: `${rect.height}%`,
              background: colorFor(category),
            }}
            onMouseEnter={() => setHovered(child)}
            onMouseLeave={() => setHovered((h) => (h?.path === child.path ? null : h))}
            onClick={() => {
              onSelect(child);
              if (child.type === "directory") onDrill(child);
            }}
            onContextMenu={(e) => {
              e.preventDefault();
              onContextMenu(child, e.clientX, e.clientY);
            }}
          >
            {wide && <span className="treemap-label">{child.name}</span>}
          </div>
        );
      })}

      {hovered && (
        <div className="treemap-tooltip">
          <div style={{ fontWeight: 600 }}>{hovered.name}</div>
          <div style={{ opacity: 0.8, fontSize: 11 }}>{hovered.path}</div>
          <div>{formatBytes(hovered.size)} ({totalSize > 0 ? ((hovered.size / totalSize) * 100).toFixed(1) : "0"}% of this view)</div>
          {hovered.type === "directory" && <div>{hovered.child_count} item(s)</div>}
          {hovered.type === "other" && <div>{hovered.child_count} smaller item(s) grouped</div>}
        </div>
      )}
    </div>
  );
}
