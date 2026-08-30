// Central platform-language + capability abstraction. Components read
// labels/capabilities from here, never from OS conditionals scattered
// through the UI, and never hardcode "Explorer"/"Finder"/"Recycle Bin"/
// "Trash" directly.

import { invoke } from "@tauri-apps/api/core";

export interface PlatformInfo {
  os: "windows" | "macos";
  os_family_label: string;
  file_manager_label: string;
  trash_label: string;
  arch: string;
}

export interface Capabilities {
  canRevealFiles: boolean;
  canTrashFiles: boolean;
  canDetectDeveloperCaches: boolean;
  canDetectAppLeftovers: boolean;
  canInspectSystemStorage: boolean;
}

let cached: PlatformInfo | null = null;

export async function getPlatformInfo(): Promise<PlatformInfo> {
  if (cached) return cached;
  cached = await invoke<PlatformInfo>("get_platform_info");
  return cached;
}

// All true on both supported platforms today -- this exists so future
// platform-conditional features (e.g. a capability only available with
// admin/Full-Disk-Access) have one place to register, rather than
// scattering `if (os === "windows")` checks through components.
export function capabilitiesFor(_platform: PlatformInfo): Capabilities {
  return {
    canRevealFiles: true,
    canTrashFiles: true,
    canDetectDeveloperCaches: true,
    canDetectAppLeftovers: true,
    canInspectSystemStorage: true,
  };
}
