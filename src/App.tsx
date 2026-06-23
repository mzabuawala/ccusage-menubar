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

// ---- Uptime data types ----

interface Outages {
  p?: number;
  m?: number;
}

interface DayStatus {
  date: string;
  outages: Outages;
  related_events: { name: string; code: string }[];
}

interface ComponentData {
  component: { code: string; name: string; startDate: string };
  days: DayStatus[];
}

interface UptimeData {
  [componentId: string]: ComponentData;
}

interface ComponentStatus {
  name: string;
  icon: string;
  latestDate: string;
}

// ---- Uptime helpers ----

function statusIcon(outages: Outages): string {
  if (outages.m !== undefined) return "🔴";
  if (outages.p !== undefined) return "🟠";
  return "🟢";
}

function latestDay(days: DayStatus[]): DayStatus | null {
  if (days.length === 0) return null;
  return [...days].sort((a, b) => b.date.localeCompare(a.date))[0];
}

function parseUptimeData(html: string): UptimeData | null {
  const marker = "var uptimeData = ";
  const markerIdx = html.indexOf(marker);
  if (markerIdx === -1) return null;

  const objStart = html.indexOf("{", markerIdx);
  if (objStart === -1) return null;

  let depth = 0;
  let objEnd = -1;
  for (let i = objStart; i < html.length; i++) {
    if (html[i] === "{") depth++;
    else if (html[i] === "}") {
      depth--;
      if (depth === 0) {
        objEnd = i;
        break;
      }
    }
  }

  if (objEnd === -1) return null;

  const objStr = html.slice(objStart, objEnd + 1);
  try {
    return JSON.parse(objStr) as UptimeData;
  } catch {
    try {
      // Safe for trusted source (status.claude.com); avoids JSON limitations
      // eslint-disable-next-line no-new-func
      return new Function(`return ${objStr}`)() as UptimeData;
    } catch {
      return null;
    }
  }
}

// ---- App ----

const money = (n: number) =>
  n.toLocaleString("en-US", { style: "currency", currency: "USD" });

function App() {
  const [stats, setStats] = useState<Stats | null>(null);
  const [claudeStatus, setClaudeStatus] = useState<ClaudeStatus | null>(null);
  const [componentStatuses, setComponentStatuses] = useState<ComponentStatus[]>([]);

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
      setClaudeStatus({ indicator: "critical", description: "Unable to fetch status" });
      await invoke("set_claude_status", { indicator: "critical" });
    }
  };

  const loadUptimeData = async () => {
    try {
      const html = await invoke<string>("fetch_status_html");
      const uptimeData = parseUptimeData(html);
      if (!uptimeData) return;

      const statuses: ComponentStatus[] = Object.values(uptimeData)
        .map((data) => {
          const day = latestDay(data.days);
          return {
            name: data.component.name,
            icon: day ? statusIcon(day.outages) : "🟢",
            latestDate: day?.date ?? "N/A",
          };
        })
        .sort((a, b) => a.name.localeCompare(b.name));

      setComponentStatuses(statuses);
    } catch (e) {
      console.error("loadUptimeData failed", e);
    }
  };

  useEffect(() => {
    load();
    loadStatus();
    loadUptimeData();
    const unlisten = listen("stats-updated", () => load());
    const statusInterval = setInterval(loadStatus, 1 * 60 * 1000);
    const uptimeInterval = setInterval(loadUptimeData, 2 * 60 * 1000);
    return () => {
      unlisten.then((f) => f());
      clearInterval(statusInterval);
      clearInterval(uptimeInterval);
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

      {componentStatuses.length > 0 && (
        <>
          <div className="divider" />
          <div className="uptime-section">
            {componentStatuses.map((cs: ComponentStatus) => (
              <div key={cs.name} className="uptime-row">
                <span className="uptime-name">{cs.name}</span>
                <span className="uptime-meta">
                  <span>{cs.icon}</span>
                  <span className="uptime-date">{cs.latestDate}</span>
                </span>
              </div>
            ))}
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
