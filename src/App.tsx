import { useEffect, useMemo, useState, type ReactNode } from "react";
import { api, type ApplyResult, type Environment, type NodeStatus, type ProbeResult, type PublicPeer } from "./api";

type BusyKey = "boot" | "status" | "peers" | "probe" | "apply" | "restart" | "open" | "setup";

const peerLimit = 12;

function App() {
  const [environment, setEnvironment] = useState<Environment | null>(null);
  const [status, setStatus] = useState<NodeStatus | null>(null);
  const [peers, setPeers] = useState<PublicPeer[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [probes, setProbes] = useState<Map<string, ProbeResult>>(new Map());
  const [busy, setBusy] = useState<Set<BusyKey>>(new Set(["boot"]));
  const [message, setMessage] = useState<string>("Starting local inspection…");
  const [error, setError] = useState<string | null>(null);
  const [useAdmin, setUseAdmin] = useState(true);
  const [restartAfterApply, setRestartAfterApply] = useState(true);
  const [copiedIpv6, setCopiedIpv6] = useState<string | null>(null);

  const configPath = environment?.configPath ?? environment?.configCandidates.find((candidate) => candidate.exists)?.path ?? "/etc/yggdrasil.conf";
  const ipv6Address = status?.ipv6Address ?? null;

  const sortedPeers = useMemo(() => {
    return [...peers].sort((left, right) => {
      if (left.up !== right.up) return left.up ? -1 : 1;
      const leftMs = left.responseMs ?? Number.MAX_SAFE_INTEGER;
      const rightMs = right.responseMs ?? Number.MAX_SAFE_INTEGER;
      if (leftMs !== rightMs) return leftMs - rightMs;
      return left.uri.localeCompare(right.uri);
    });
  }, [peers]);

  const selectedPeers = useMemo(() => sortedPeers.filter((peer) => selected.has(peer.uri)), [selected, sortedPeers]);

  useEffect(() => {
    void boot();
  }, []);

  useEffect(() => {
    setCopiedIpv6(null);
  }, [ipv6Address]);

  async function boot() {
    await withBusy("boot", async () => {
      setError(null);
      const [environmentResult, statusResult, peersResult] = await Promise.allSettled([
        api.detectEnvironment(),
        api.getNodeStatus(false),
        api.fetchPublicPeers(),
      ]);

      const bootErrors: string[] = [];
      const nextEnvironment = environmentResult.status === "fulfilled" ? environmentResult.value : null;
      const nextStatus = statusResult.status === "fulfilled" ? statusResult.value : null;
      const nextPeers = peersResult.status === "fulfilled" ? peersResult.value : [];

      if (nextEnvironment) setEnvironment(nextEnvironment);
      else if (environmentResult.status === "rejected") bootErrors.push(`Environment check failed: ${toErrorMessage(environmentResult.reason)}`);

      if (nextStatus) setStatus(nextStatus);
      else if (statusResult.status === "rejected") bootErrors.push(`Node status check failed: ${toErrorMessage(statusResult.reason)}`);

      if (peersResult.status === "fulfilled") setPeers(nextPeers);
      else bootErrors.push(`Public peer fetch failed: ${toErrorMessage(peersResult.reason)}`);

      const fastest = [...nextPeers]
        .filter((peer) => peer.up)
        .sort((left, right) => (left.responseMs ?? Number.MAX_SAFE_INTEGER) - (right.responseMs ?? Number.MAX_SAFE_INTEGER))
        .slice(0, 6)
        .map((peer) => peer.uri);
      setSelected(new Set(fastest));
      if (nextEnvironment) setUseAdmin(nextEnvironment.configCandidates.some((candidate) => candidate.exists && !candidate.writable));
      setMessage(`Loaded ${nextPeers.length} public peers. Selected fastest ${fastest.length}.`);
      if (bootErrors.length) setError(bootErrors.join("\n"));
    });
  }

  async function refreshStatus(admin = false) {
    await withBusy("status", async () => {
      setStatus(await api.getNodeStatus(admin));
      setMessage(admin ? "Status refreshed with admin prompt." : "Status refreshed.");
    });
  }

  async function refreshPeers() {
    await withBusy("peers", async () => {
      const nextPeers = await api.fetchPublicPeers();
      setPeers(nextPeers);
      setMessage(`Fetched ${nextPeers.length} public peers.`);
    });
  }

  async function probeSelected() {
    await withBusy("probe", async () => {
      const targetUris = selectedPeers.slice(0, peerLimit).map((peer) => peer.uri);
      const results = await api.probePeers(targetUris);
      setProbes(new Map(results.map((result) => [result.uri, result])));
      setMessage(`Probed ${results.length} selected peers from this Mac.`);
    });
  }

  async function applySelectedPeers() {
    await withBusy("apply", async () => {
      const result = await api.applyPeersToConfig(configPath, selectedPeers.map((peer) => peer.uri), useAdmin, restartAfterApply);
      setStatus(await api.getNodeStatus(false));
      setMessage(formatApplyResult(result));
    });
  }

  async function restartDaemon() {
    await withBusy("restart", async () => {
      const result = await api.restartYggdrasil(useAdmin);
      setStatus(await api.getNodeStatus(false));
      setMessage(result.message);
    });
  }

  async function openConfigLocation() {
    await withBusy("open", async () => {
      const result = await api.openConfigLocation(configPath);
      setMessage(result.message);
    });
  }

  async function installYggdrasil() {
    await withBusy("setup", async () => {
      const result = await api.installYggdrasilWithBrew();
      setEnvironment(await api.detectEnvironment());
      setMessage(result.message);
    });
  }

  async function createConfig() {
    await withBusy("setup", async () => {
      const result = await api.createDefaultConfig(configPath, useAdmin);
      setEnvironment(await api.detectEnvironment());
      setMessage(result.message);
    });
  }

  async function installAutostart() {
    await withBusy("setup", async () => {
      const result = await api.installLaunchdService(configPath, useAdmin);
      setEnvironment(await api.detectEnvironment());
      setStatus(await api.getNodeStatus(false));
      setMessage(result.message);
    });
  }

  async function copyIpv6Address() {
    if (!ipv6Address) return;
    setError(null);

    try {
      await navigator.clipboard.writeText(ipv6Address);
      setCopiedIpv6(ipv6Address);
      setMessage("Copied IPv6 address to clipboard.");
    } catch (unknownError) {
      const nextError = unknownError instanceof Error ? unknownError.message : String(unknownError);
      setError(nextError);
      setMessage("Could not copy IPv6 address.");
    }
  }

  async function withBusy(key: BusyKey, task: () => Promise<void>) {
    setBusy((current) => new Set(current).add(key));
    setError(null);
    try {
      await task();
    } catch (unknownError) {
      const nextError = unknownError instanceof Error ? unknownError.message : String(unknownError);
      setError(nextError);
      setMessage("Action failed. Details shown below.");
    } finally {
      setBusy((current) => {
        const next = new Set(current);
        next.delete(key);
        return next;
      });
    }
  }

  function selectFastest() {
    setSelected(new Set(sortedPeers.filter((peer) => peer.up).slice(0, 6).map((peer) => peer.uri)));
    setMessage("Selected fastest live peers from public registry.");
  }

  function togglePeer(uri: string) {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(uri)) next.delete(uri);
      else next.add(uri);
      return next;
    });
  }

  const health = status?.adminApiOk ? "connected" : environment?.processRunning ? "daemon-only" : "offline";
  const isBusy = (key: BusyKey) => busy.has(key);

  return (
    <main className="shell">
      <section className="hero panel">
        <div className="hero-copy">
          <p className="eyebrow">Yggdrasil local control panel</p>
          <h1>Turn this Mac into a reachable Yggdrasil node.</h1>
          <p className="lede">Manage public peers, inspect daemon health, and update the local config without hunting through `/etc` by hand.</p>
        </div>
        <div className={`orb orb-${health}`} aria-label={`Health: ${health}`}>
          <span>{health === "connected" ? "Online" : health === "daemon-only" ? "Daemon" : "Offline"}</span>
        </div>
      </section>

      <section className="status-grid">
        <StatusCard label="Daemon" value={environment?.processRunning ? "running" : "not running"} tone={environment?.processRunning ? "good" : "bad"} detail={environment?.launchd.found ? `launchd: ${environment.launchd.state ?? "unknown"}` : "launchd label not found"} />
        <StatusCard label="Admin API" value={status?.adminApiOk ? "available" : "restricted"} tone={status?.adminApiOk ? "good" : "warn"} detail={status?.error ?? "yggdrasilctl can query node state"} />
        <StatusCard label="Peers" value={status ? String(status.peerCount) : "unknown"} tone={status?.peerCount ? "good" : "neutral"} detail="active sessions reported by yggdrasilctl" />
        <StatusCard label="Config" value={configPath} tone="neutral" detail={environment?.configCandidates.find((candidate) => candidate.path === configPath)?.writable ? "writable by current user" : "admin rights likely needed"} />
      </section>

      <section className="workspace">
        <div className="panel node-panel">
          <div className="section-heading">
            <div>
              <p className="eyebrow">Node identity</p>
              <h2>Local Yggdrasil state</h2>
            </div>
            <div className="actions compact">
              <button onClick={() => void refreshStatus(false)} disabled={isBusy("status")}>Refresh</button>
              <button className="secondary" onClick={() => void refreshStatus(true)} disabled={isBusy("status")}>Admin refresh</button>
            </div>
          </div>

          <dl className="facts">
            <Fact label="IPv6" value={ipv6Address ?? "Needs admin API access"}>
              {ipv6Address && (
                <button className="inline-action" onClick={() => void copyIpv6Address()} aria-label="Copy IPv6 address">
                  {copiedIpv6 === ipv6Address ? "Copied" : "Copy"}
                </button>
              )}
            </Fact>
            <Fact label="Subnet" value={status?.subnet ?? "Unknown"} />
            <Fact label="Coords" value={status?.coords ?? "Unknown"} />
            <Fact label="Version" value={[status?.buildName, status?.buildVersion].filter(Boolean).join(" ") || environment?.yggdrasil.version || "Unknown"} />
            <Fact label="Binary" value={environment?.yggdrasil.path ?? "Not found"} />
            <Fact label="Control" value={environment?.yggdrasilctl.path ?? "Not found"} />
          </dl>

          <div className="config-actions">
            <div className="setup-strip">
              <p className="eyebrow">First-run setup</p>
              <div className="actions">
                <button onClick={() => void installYggdrasil()} disabled={isBusy("setup")}>Install via Homebrew</button>
                <button onClick={() => void createConfig()} disabled={isBusy("setup")}>Create config</button>
                <button onClick={() => void installAutostart()} disabled={isBusy("setup")}>Install/repair autostart</button>
              </div>
            </div>
            <label className="toggle">
              <input type="checkbox" checked={useAdmin} onChange={(event) => setUseAdmin(event.target.checked)} />
              <span>Use macOS admin prompt for protected daemon/config actions</span>
            </label>
            <label className="toggle">
              <input type="checkbox" checked={restartAfterApply} onChange={(event) => setRestartAfterApply(event.target.checked)} />
              <span>Restart daemon after applying peers</span>
            </label>
            <div className="actions">
              <button onClick={() => void openConfigLocation()} disabled={isBusy("open")}>Reveal config</button>
              <button onClick={() => void restartDaemon()} disabled={isBusy("restart")}>Restart daemon</button>
            </div>
          </div>
        </div>

        <div className="panel action-panel">
          <div className="section-heading">
            <div>
              <p className="eyebrow">Peer import</p>
              <h2>{selectedPeers.length} selected</h2>
            </div>
            <span className="pill">{peers.filter((peer) => peer.up).length} live in registry</span>
          </div>
          <p className="muted">The app backs up the config before writing. It normalises Yggdrasil config to JSON so future edits are machine-safe.</p>
          <div className="actions stacked">
            <button onClick={selectFastest} disabled={!peers.length}>Select fastest 6</button>
            <button onClick={() => void probeSelected()} disabled={!selectedPeers.length || isBusy("probe")}>Probe selected from this Mac</button>
            <button className="primary" onClick={() => void applySelectedPeers()} disabled={!selectedPeers.length || isBusy("apply")}>Apply selected peers</button>
            <button className="secondary" onClick={() => void refreshPeers()} disabled={isBusy("peers")}>Reload public list</button>
          </div>
        </div>
      </section>

      <section className="panel peer-panel">
        <div className="section-heading">
          <div>
            <p className="eyebrow">Public peers</p>
            <h2>Fastest registry entries</h2>
          </div>
          <span className="pill">showing {Math.min(sortedPeers.length, 80)} / {sortedPeers.length}</span>
        </div>

        <div className="peer-table" role="table" aria-label="Public peers">
          <div className="peer-row peer-head" role="row">
            <span>Use</span>
            <span>URI</span>
            <span>Region</span>
            <span>Registry</span>
            <span>Local probe</span>
          </div>
          {sortedPeers.slice(0, 80).map((peer) => {
            const probe = probes.get(peer.uri);
            return (
              <label className="peer-row" role="row" key={peer.uri}>
                <span>
                  <input type="checkbox" checked={selected.has(peer.uri)} onChange={() => togglePeer(peer.uri)} />
                </span>
                <span className="uri"><strong>{peer.scheme}</strong>{peer.uri.replace(`${peer.scheme}://`, "://")}</span>
                <span>{peer.region}</span>
                <span className={peer.up ? "good-text" : "bad-text"}>{peer.up ? `${peer.responseMs ?? "?"} ms` : "down"}</span>
                <span className={probe?.ok ? "good-text" : probe ? "bad-text" : "muted"}>{probe ? probe.localMs !== null ? `${probe.localMs} ms` : probe.message : "not probed"}</span>
              </label>
            );
          })}
        </div>
      </section>

      <section className="console panel" aria-live="polite">
        <span className="dot" />
        <p>{message}</p>
        {busy.size > 0 && <p className="muted">Working: {[...busy].join(", ")}</p>}
        {error && <pre>{error}</pre>}
        {environment?.notes.map((note) => <p className="muted" key={note}>{note}</p>)}
      </section>
    </main>
  );
}

function StatusCard({ label, value, detail, tone }: { label: string; value: string; detail: string; tone: "good" | "warn" | "bad" | "neutral" }) {
  return (
    <article className={`status-card tone-${tone}`}>
      <p>{label}</p>
      <strong>{value}</strong>
      <span>{detail}</span>
    </article>
  );
}

function Fact({ label, value, children }: { label: string; value: string; children?: ReactNode }) {
  return (
    <div>
      <dt>{label}</dt>
      <dd>
        <span>{value}</span>
        {children}
      </dd>
    </div>
  );
}

function formatApplyResult(result: ApplyResult) {
  return result.backupPath ? `${result.message} Backup: ${result.backupPath}` : result.message;
}

function toErrorMessage(reason: unknown) {
  return reason instanceof Error ? reason.message : String(reason);
}

export default App;
