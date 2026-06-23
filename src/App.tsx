import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

interface DayStat {
  name: string;
  cost: number;
  isToday: boolean;
}

interface ClaudeStatus {
  indicator: "none" | "minor" | "major" | "critical";
  description: string;
}

interface Stats {
  available: boolean;
  hasData: boolean;
  monthTotal: number;
  today: number;
  weekTotal: number;
  days: DayStat[];
}

const money = (n: number) =>
  n.toLocaleString("en-US", { style: "currency", currency: "USD" });

function App() {
  const [stats, setStats] = useState<Stats | null>(null);
  const [claudeStatus, setClaudeStatus] = useState<ClaudeStatus | null>(null);

  const load = async () => {
    try {
      setStats(await invoke<Stats>("get_stats"));
    } catch (e) {
      console.error("get_stats failed", e);
    }
  };

  const loadStatus = async () => {
    try {
      const res = await fetch("https://status.claude.com/api/v2/status.json");
      const data = await res.json();
      const status: ClaudeStatus = data.status;
      setClaudeStatus(status);
      await invoke("set_claude_status", { indicator: status.indicator });
    } catch {
      // http request failed, set status to critical
      setClaudeStatus({ indicator: "critical", description: "Unable to fetch status" });
      await invoke("set_claude_status", { indicator: "critical" });
    }
  };

  useEffect(() => {
    load();
    loadStatus();
    const unlisten = listen("stats-updated", () => load());
    const interval = setInterval(loadStatus, 1 * 60 * 1000);
    return () => {
      unlisten.then((f) => f());
      clearInterval(interval);
    };
  }, []);

  return (
    <div className="card">
      {!stats ? (
        <div className="muted">Loading…</div>
      ) : !stats.hasData ? (
        <div className="muted">Loading…</div>
      ) : !stats.available ? (
        <div className="error">
          <div>ccusage not found</div>
          <div className="hint">npm install -g ccusage</div>
        </div>
      ) : (
        <>
          <div className="totals">
            <Row label="This Month" value={stats.monthTotal} strong />
            <Row label="This Week" value={stats.weekTotal} strong />
            <Row label="Today" value={stats.today} strong />
          </div>

          <div className="divider" />

          <div className="days">
            {stats.days.map((d) => (
              <Row
                key={d.name}
                label={d.name}
                value={d.cost}
                today={d.isToday}
                dim={d.cost === 0}
              />
            ))}
          </div>
        </>
      )}

      {claudeStatus && (
        <>
          <div className="divider" />
          <div className={`claude-status indicator-${claudeStatus.indicator}`}>
            <span className="status-dot" />
            <span className="status-text">{claudeStatus.description}</span>
          </div>
        </>
      )}

      <div className="divider" />

      <div className="actions">
        <button onClick={() => invoke("refresh_now")}>Refresh</button>
        <button onClick={() => invoke("quit_app")}>Quit</button>
      </div>
    </div>
  );
}

function Row({
  label,
  value,
  strong,
  today,
  dim,
}: {
  label: string;
  value: number;
  strong?: boolean;
  today?: boolean;
  dim?: boolean;
}) {
  return (
    <div
      className={`row${strong ? " strong" : ""}${today ? " today" : ""}${dim ? " dim" : ""}`}
    >
      <span className="label">{label}</span>
      <span className="value">{money(value)}</span>
    </div>
  );
}

export default App;
