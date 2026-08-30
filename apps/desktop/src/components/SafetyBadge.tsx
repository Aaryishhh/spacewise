import { SafetyLevel } from "../api";

const STYLES: Record<SafetyLevel, { cls: string; label: string }> = {
  Safe: { cls: "badge-safe", label: "Safe" },
  Review: { cls: "badge-review", label: "Review" },
  Advanced: { cls: "badge-advanced", label: "Advanced" },
  NeverAutoDelete: { cls: "badge-never", label: "Never Auto-Delete" },
};

export default function SafetyBadge({ level }: { level: SafetyLevel }) {
  const style = STYLES[level];
  return <span className={`badge ${style.cls}`}>{style.label}</span>;
}
