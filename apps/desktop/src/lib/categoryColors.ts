// Decoupled from rendering on purpose: Treemap.tsx only ever calls
// categoryFor()/colorFor() and never hardcodes a color -- swapping the
// palette or the classification rules later needs no rendering change.

export type StorageCategoryKey =
  | "applications"
  | "developer"
  | "video"
  | "images"
  | "documents"
  | "archives"
  | "temporary"
  | "system"
  | "directory"
  | "other";

const EXTENSION_MAP: Record<string, StorageCategoryKey> = {
  exe: "applications", msi: "applications", app: "applications", dmg: "applications",
  rs: "developer", ts: "developer", tsx: "developer", js: "developer", jsx: "developer",
  py: "developer", go: "developer", java: "developer", c: "developer", cpp: "developer",
  h: "developer", swift: "developer", json: "developer", toml: "developer", lock: "developer",
  mp4: "video", mov: "video", mkv: "video", avi: "video", webm: "video",
  jpg: "images", jpeg: "images", png: "images", gif: "images", webp: "images", heic: "images", svg: "images",
  pdf: "documents", doc: "documents", docx: "documents", txt: "documents", md: "documents",
  xls: "documents", xlsx: "documents", ppt: "documents", pptx: "documents",
  zip: "archives", rar: "archives", "7z": "archives", tar: "archives", gz: "archives",
  tmp: "temporary", temp: "temporary", cache: "temporary", log: "temporary",
  dll: "system", sys: "system", dat: "system",
};

const COLORS: Record<StorageCategoryKey, string> = {
  applications: "#5b8dff",
  developer: "#f2a33c",
  video: "#e05fa0",
  images: "#39c2c9",
  documents: "#8b7cf6",
  archives: "#b08968",
  temporary: "#9aa0ac",
  system: "#e05656",
  directory: "#4a7dde",
  other: "#6b6b73",
};

export function categoryFor(name: string, type: "directory" | "file" | "other"): StorageCategoryKey {
  if (type === "directory") return "directory";
  if (type === "other") return "other";
  const ext = name.includes(".") ? name.split(".").pop()?.toLowerCase() : undefined;
  if (ext && EXTENSION_MAP[ext]) return EXTENSION_MAP[ext];
  return "other";
}

export function colorFor(category: StorageCategoryKey): string {
  return COLORS[category] ?? COLORS.other;
}
