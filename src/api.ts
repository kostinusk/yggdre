import { invoke } from "@tauri-apps/api/core";

export type BinaryInfo = {
  path: string | null;
  version: string | null;
};

export type ConfigCandidate = {
  path: string;
  exists: boolean;
  readable: boolean;
  writable: boolean;
  parentWritable: boolean;
};

export type LaunchdStatus = {
  label: string;
  found: boolean;
  state: string | null;
  pid: number | null;
  path: string | null;
};

export type Environment = {
  os: string;
  arch: string;
  yggdrasil: BinaryInfo;
  yggdrasilctl: BinaryInfo;
  configPath: string | null;
  configCandidates: ConfigCandidate[];
  processRunning: boolean;
  launchd: LaunchdStatus;
  notes: string[];
};

export type NodeStatus = {
  processRunning: boolean;
  adminApiOk: boolean;
  error: string | null;
  buildName: string | null;
  buildVersion: string | null;
  ipv6Address: string | null;
  subnet: string | null;
  publicKey: string | null;
  coords: string | null;
  peerCount: number;
  selfInfo: unknown | null;
};

export type PublicPeer = {
  uri: string;
  source: string;
  region: string;
  scheme: string;
  up: boolean;
  key: string | null;
  responseMs: number | null;
  lastSeen: number | null;
  protoMinor: number | null;
  priority: number | null;
};

export type ProbeResult = {
  uri: string;
  ok: boolean;
  localMs: number | null;
  message: string;
};

export type ApplyResult = {
  backupPath: string | null;
  message: string;
};

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

const isTauri = () => typeof window !== "undefined" && window.__TAURI_INTERNALS__ !== undefined;

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (isTauri()) {
    return invoke<T>(command, args);
  }

  return previewCall<T>(command, args);
}

export const api = {
  detectEnvironment: () => call<Environment>("detect_environment"),
  getNodeStatus: (useAdmin: boolean) => call<NodeStatus>("get_node_status", { useAdmin }),
  fetchPublicPeers: () => call<PublicPeer[]>("fetch_public_peers"),
  probePeers: (uris: string[]) => call<ProbeResult[]>("probe_peers", { uris }),
  applyPeersToConfig: (configPath: string, peers: string[], useAdmin: boolean, restart: boolean) =>
    call<ApplyResult>("apply_peers_to_config", { configPath, peers, useAdmin, restart }),
  restartYggdrasil: (useAdmin: boolean) => call<ApplyResult>("restart_yggdrasil", { useAdmin }),
  installYggdrasilWithBrew: () => call<ApplyResult>("install_yggdrasil_with_brew"),
  createDefaultConfig: (configPath: string, useAdmin: boolean) => call<ApplyResult>("create_default_config", { configPath, useAdmin }),
  installLaunchdService: (configPath: string, useAdmin: boolean) => call<ApplyResult>("install_launchd_service", { configPath, useAdmin }),
  openConfigLocation: (path: string) => call<ApplyResult>("open_config_location", { path }),
};

async function previewCall<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  await new Promise((resolve) => window.setTimeout(resolve, 220));

  if (command === "detect_environment") {
    return {
      os: "macos-preview",
      arch: "arm64",
      yggdrasil: { path: "/usr/local/bin/yggdrasil", version: "Build version: 0.5.13" },
      yggdrasilctl: { path: "/usr/local/bin/yggdrasilctl", version: "Build version: 0.5.13" },
      configPath: "/etc/yggdrasil.conf",
      configCandidates: [
        { path: "/etc/yggdrasil.conf", exists: true, readable: false, writable: false, parentWritable: false },
      ],
      processRunning: true,
      launchd: { label: "yggdrasil", found: true, state: "running", pid: 575, path: "/Library/LaunchDaemons/yggdrasil.plist" },
      notes: ["Preview mode: open inside Tauri to use real system commands."],
    } satisfies Environment as T;
  }

  if (command === "get_node_status") {
    return {
      processRunning: true,
      adminApiOk: false,
      error: args?.useAdmin ? null : "Admin socket needs elevated access on this Mac.",
      buildName: args?.useAdmin ? "yggdrasil" : null,
      buildVersion: args?.useAdmin ? "0.5.13" : null,
      ipv6Address: args?.useAdmin ? "200:1234:abcd:preview::1" : null,
      subnet: args?.useAdmin ? "300:1234:abcd:preview::/64" : null,
      publicKey: null,
      coords: args?.useAdmin ? "[1, 42, 7]" : null,
      peerCount: args?.useAdmin ? 4 : 0,
      selfInfo: null,
    } satisfies NodeStatus as T;
  }

  if (command === "fetch_public_peers") {
    return [
      { uri: "tcp://94.159.110.4:65535", source: "germany.md", region: "germany", scheme: "tcp", up: true, key: "0000006f...", responseMs: 11, lastSeen: null, protoMinor: 5, priority: null },
      { uri: "tls://37.205.14.171:993?key=0009e16b", source: "czechia.md", region: "czechia", scheme: "tls", up: true, key: "0009e16b...", responseMs: 22, lastSeen: null, protoMinor: 5, priority: null },
      { uri: "quic://ygg2.mk16.de:1339?key=000000d8", source: "germany.md", region: "germany", scheme: "quic", up: true, key: "000000d8...", responseMs: 39, lastSeen: null, protoMinor: 5, priority: null },
      { uri: "tls://offline.example:443", source: "sample.md", region: "sample", scheme: "tls", up: false, key: null, responseMs: null, lastSeen: null, protoMinor: null, priority: null },
    ] satisfies PublicPeer[] as T;
  }

  if (command === "probe_peers") {
    const uris = Array.isArray(args?.uris) ? args.uris.filter((value): value is string => typeof value === "string") : [];
    return uris.map((uri, index) => ({ uri, ok: index % 3 !== 2, localMs: index % 3 !== 2 ? 30 + index * 14 : null, message: index % 3 !== 2 ? "TCP reachable" : "Preview: skipped" })) satisfies ProbeResult[] as T;
  }

  if (command === "apply_peers_to_config") {
    return { backupPath: "/etc/yggdrasil.conf.yggdre-backup.preview", message: "Preview: would update Peers and restart daemon." } satisfies ApplyResult as T;
  }

  if (command === "restart_yggdrasil" || command === "open_config_location") {
    return { backupPath: null, message: "Preview: command accepted." } satisfies ApplyResult as T;
  }

  if (command === "install_yggdrasil_with_brew") {
    return { backupPath: null, message: "Preview: Yggdrasil already installed at /usr/local/bin/yggdrasil." } satisfies ApplyResult as T;
  }

  if (command === "create_default_config") {
    return { backupPath: null, message: "Preview: config already exists." } satisfies ApplyResult as T;
  }

  if (command === "install_launchd_service") {
    return { backupPath: null, message: "Preview: launchd daemon installed/repaired." } satisfies ApplyResult as T;
  }

  throw new Error(`Preview command not implemented: ${command}`);
}
