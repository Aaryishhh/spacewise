import { invoke } from "@tauri-apps/api/core";

export type ScanStatus = "idle" | "starting" | "scanning" | "cancelling" | "completed" | "failed" | "cancelled";

export interface SkippedItem {
  path: string | null;
  reason: string;
}

export interface ScanStats {
  files_scanned: number;
  dirs_scanned: number;
  total_logical_size: number;
  errors: number;
  skipped_total: number;
  skipped_sample: SkippedItem[];
  duration_ms: number;
  cancelled: boolean;
}

export interface ScanSummary {
  scan_id: string;
  stats: ScanStats;
  total_size: number;
  status: ScanStatus;
}

export interface ScanStatusPayload {
  status: ScanStatus;
  scan_id: string | null;
  root: string | null;
}

export interface ScanProgressPayload {
  files_scanned: number;
  dirs_scanned: number;
  total_logical_size: number;
  skipped_total: number;
  current_path: string | null;
  elapsed_ms: number;
  files_per_sec: number;
  mb_per_sec: number;
}

export interface CategoryTotal {
  category_id: string;
  total_size: number;
  item_count: number;
}

export interface DashboardData {
  scan_id: string;
  root: string;
  total_size: number;
  scanned_at: string | null;
  category_totals: CategoryTotal[];
}

export interface DirectoryAggregate {
  path: string;
  total_size: number;
  allocated_size: number;
  file_count: number;
  dir_count: number;
  latest_modified: string | null;
}

export interface FileEntry {
  id: string;
  scan_id: string;
  path: string;
  parent: string | null;
  logical_size: number;
  allocated_size: number;
  extension: string | null;
  created_at: string | null;
  modified_at: string | null;
  accessed_at: string | null;
  is_dir: boolean;
  is_symlink: boolean;
  is_hardlink: boolean;
  is_hidden: boolean;
  is_system: boolean;
  filesystem_id: string | null;
}

export type SafetyLevel = "Safe" | "Review" | "Advanced" | "NeverAutoDelete";

export interface CleanupCandidate {
  id: string;
  category_id: string;
  display_name: string;
  paths: string[];
  total_size: number;
  safety: SafetyLevel;
}

export interface Recommendation {
  candidate: CleanupCandidate;
  score: number;
  rationale: string;
}

export interface DuplicateGroup {
  id: string;
  size: number;
  content_hash: string;
  paths: string[];
}

export interface CleanupAction {
  id: string;
  performed_at: string;
  category_id: string;
  paths: string[];
  bytes_freed: number;
  undoable: boolean;
}

export interface CleanupOutcome {
  succeeded: CleanupAction[];
  failed: string[];
}

export interface CategoryDelta {
  category_id: string;
  delta_bytes: number;
}

export interface GrowthSummary {
  growth_bytes: number;
  period_days: number;
  top_contributors: CategoryDelta[];
}

export interface InstalledApp {
  name: string;
  publisher: string | null;
  install_location: string | null;
  estimated_size_bytes: number | null;
  uninstall_command: string | null;
}

export interface AppAssociation {
  app: InstalledApp;
  associated_paths: string[];
  associated_size: number;
  confidence: number;
}

export const api = {
  runScan: (root: string) => invoke<ScanSummary>("run_scan", { root }),
  cancelScan: () => invoke<void>("cancel_scan"),
  getScanStatus: () => invoke<ScanStatusPayload>("get_scan_status"),
  getDashboard: () => invoke<DashboardData | null>("get_dashboard"),
  getDirectoryChildren: (scanId: string, path: string) =>
    invoke<DirectoryAggregate[]>("get_directory_children", { scanId, path }),
  getLargeFiles: (scanId: string, minSizeBytes: number, olderThanDays?: number) =>
    invoke<FileEntry[]>("get_large_files", { scanId, minSizeBytes, olderThanDays: olderThanDays ?? null }),
  getRecommendations: (scanId: string) => invoke<Recommendation[]>("get_recommendations", { scanId }),
  getDuplicates: (scanId: string) => invoke<DuplicateGroup[]>("get_duplicates", { scanId }),
  executeCleanup: (candidate: CleanupCandidate) => invoke<CleanupOutcome>("execute_cleanup", { candidate }),
  getCleanupHistory: () => invoke<CleanupAction[]>("get_cleanup_history"),
  getGrowthSummary: (lookbackDays: number) => invoke<GrowthSummary | null>("get_growth_summary", { lookbackDays }),
  listInstalledApps: () => invoke<InstalledApp[]>("list_installed_apps"),
  getAppAssociations: (scanId: string) => invoke<AppAssociation[]>("get_app_associations", { scanId }),
  getDeveloperStorage: (scanId: string) => invoke<CategoryTotal[]>("get_developer_storage", { scanId }),
  revealPath: (path: string) => invoke<void>("reveal_path", { path }),
};

export function formatBytes(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let size = bytes;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${size.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
}
