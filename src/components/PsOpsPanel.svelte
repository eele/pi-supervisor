<script lang="ts">
  import { getPsOps, getPsOpDetail } from "../api";
  import type { PsOpFull, PsOpRow } from "../types";

  interface Meta {
    total: number;
    from: number;
    to: number;
    q: string;
  }
  interface Props {
    /** Tab is visible — refresh pauses otherwise. */
    active: boolean;
    /** Statusbar summary (PS Audit: N records · showing x–y · filter …). */
    onMeta?: (m: Meta | null) => void;
  }
  let { active, onMeta }: Props = $props();

  const PAGE_SIZE = 50;

  let qInput = $state("");
  let q = $state("");
  let page = $state(1);
  let total = $state(0);
  let rows = $state<PsOpRow[]>([]);
  let expandedId = $state<number | null>(null);
  let details = $state<Record<number, PsOpFull>>({});
  let focused = $state(false);
  let error = $state("");
  let fresh = $state<Set<number>>(new Set());
  let freshTimer: ReturnType<typeof setTimeout> | undefined;

  const pages = $derived(Math.max(1, Math.ceil(total / PAGE_SIZE)));

  // Search: 300 ms debounce, any new filter resets to page 1.
  $effect(() => {
    const v = qInput.trim();
    const t = setTimeout(() => {
      q = v;
      page = 1;
    }, 300);
    return () => clearTimeout(t);
  });

  let lastMaxId = 0;
  function apply(r: { total: number; rows: PsOpRow[] }, silent: boolean) {
    const seen = lastMaxId;
    const maxId = r.rows.reduce((m, x) => Math.max(m, x.id), 0);
    if (seen > 0 && maxId > seen) {
      const ids = new Set(r.rows.filter((x) => x.id > seen).map((x) => x.id));
      fresh = ids;
      clearTimeout(freshTimer);
      freshTimer = setTimeout(() => (fresh = new Set()), 2500);
    }
    lastMaxId = Math.max(seen, maxId);
    void silent;
    total = r.total;
    rows = r.rows;
    error = "";
  }

  // (q, page) change → fetch the page.
  $effect(() => {
    const qq = q,
      pp = page;
    let dead = false;
    getPsOps(qq, pp, PAGE_SIZE)
      .then((r) => {
        if (!dead) apply(r, false);
      })
      .catch((e) => {
        if (!dead) error = e instanceof Error ? e.message : String(e);
      });
    return () => {
      dead = true;
    };
  });

  // 5 s auto-refresh of the current page; paused while the search box has
  // focus or a row is expanded (the user is reading, not browsing).
  $effect(() => {
    if (!active || focused || expandedId !== null) return;
    const qq = q,
      pp = page;
    const tick = () => {
      getPsOps(qq, pp, PAGE_SIZE)
        .then((r) => apply(r, true))
        .catch(() => {});
    };
    // Refresh once on activation (tab switch) before settling into 5 s polls.
    tick();
    const t = setInterval(tick, 5000);
    return () => clearInterval(t);
  });

  $effect(() => () => clearTimeout(freshTimer));

  // Statusbar summary while this tab is active.
  $effect(() => {
    if (!onMeta) return;
    if (!active) {
      onMeta(null);
      return;
    }
    const base = (page - 1) * PAGE_SIZE;
    onMeta({ total, from: rows.length ? base + 1 : 0, to: base + rows.length, q });
  });

  async function toggleRow(r: PsOpRow) {
    if (expandedId === r.id) {
      expandedId = null;
      return;
    }
    expandedId = r.id;
    if (!details[r.id]) {
      try {
        const d = await getPsOpDetail(r.id);
        if (d) details[r.id] = d;
      } catch {
        /* the block keeps showing "Loading…" — retry on next click */
      }
    }
  }

  // Orange "…": native text-overflow ellipsis cannot be recolored, so the
  // preview span stays single-line and we detect overflow ourselves and
  // toggle .clipped (which reveals the sibling marker). ResizeObserver
  // recomputes on window resize / panel re-layout.
  function overflow(node: HTMLElement) {
    const update = () => {
      node.classList.toggle("clipped", node.scrollWidth > node.clientWidth + 1);
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(node);
    return { destroy: () => ro.disconnect() };
  }

  function fmtTime(ts: string): string {
    // 2026-10-26T09:41:02Z → 10-26 09:41:02
    return ts.length >= 19 ? `${ts.slice(5, 10)} ${ts.slice(11, 19)}` : ts;
  }
  function fmtDur(ms: number | null): string {
    if (ms === null) return "—";
    const s = ms / 1000;
    if (s < 1) return `${s.toFixed(2)}s`;
    if (s < 10) return `${s.toFixed(1)}s`;
    return `${Math.round(s)}s`;
  }
  function chipClass(st: string): string {
    if (st === "ok") return "ok";
    if (st === "timeout") return "warn";
    if (["auth_failed", "exec_failed", "log_failed"].includes(st)) return "err";
    if (st === "started") return "info";
    return "gray"; // client_disconnected | disabled | unknown
  }
  function chipLabel(st: string): string {
    return st === "client_disconnected" ? "client_disc." : st;
  }
  function pageWindow(): number[] {
    const start = Math.max(1, Math.min(page - 1, pages - 2));
    const end = Math.min(pages, start + 2);
    const out: number[] = [];
    for (let n = start; n <= end; n++) out.push(n);
    return out;
  }
</script>

<div class="psop-page">
  <div class="psop-searchrow">
    <div class="psop-searchbox">
      <span class="psop-search-icon" aria-hidden="true"></span>
      <input
        class="psop-search"
        data-testid="psop-search"
        type="text"
        placeholder="Search command / stdout / stderr…"
        aria-label="Search command records"
        bind:value={qInput}
        onfocus={() => (focused = true)}
        onblur={() => (focused = false)}
      />
    </div>
    <span class="psop-search-hint">match: command / stdout / stderr · 300 ms debounce</span>
  </div>

  <div class="psop-head">
    <span>Time (UTC)</span>
    <span>Status</span>
    <span>Exit</span>
    <span>Dur</span>
    <span>Command / Output preview · single line</span>
  </div>

  <div class="psop-list" data-testid="psop-list">
    {#each rows as r (r.id)}
      <div
        class="psop-row"
        class:expanded={expandedId === r.id}
        class:new={fresh.has(r.id)}
        data-testid="psop-row-{r.id}"
        role="button"
        tabindex="0"
        onclick={() => toggleRow(r)}
        onkeydown={(e) => e.key === "Enter" && toggleRow(r)}
      >
        <span class="psop-time">{fmtTime(r.ts)}</span>
        <span class="psop-chip {chipClass(r.status)}">{chipLabel(r.status)}</span>
        <span class="psop-exit">{r.exit_code === null ? "—" : r.exit_code}</span>
        <span class="psop-dur">{fmtDur(r.duration_ms)}</span>
        <span class="psop-prev">
          <span class="psop-prev-text" use:overflow title={r.preview || r.error || ""}
            >{expandedId === r.id ? "▾ " : "› "}{r.preview || (r.error ?? "—")}</span
          ><span class="psop-ellipsis" aria-hidden="true">…</span>
        </span>
      </div>
      {#if expandedId === r.id}
        {@const d = details[r.id]}
        <div class="psop-detail" data-testid="psop-detail">
          {#if d}
            <div class="psop-meta">
              {d.request_id ?? "—"} · {d.client ?? "—"} · stream {d.stream ? "on" : "off"} ·
              {d.method}
            </div>
            <div class="psop-sect">COMMAND (args_json.script · full text)</div>
            <pre class="psop-pre">{d.command || "—"}</pre>
            <div class="psop-sect">
              STDOUT {d.stdout_truncated ? "(full · truncated at 1 MB mark)" : "(full)"}
            </div>
            <pre class="psop-pre">{d.stdout || "—"}</pre>
            <div class="psop-sect">STDERR (full)</div>
            <pre class="psop-pre err">{d.stderr || "—"}</pre>
            {#if d.error}
              <div class="psop-errline">{d.error}</div>
            {/if}
          {:else}
            <div class="psop-loading">Loading full text…</div>
          {/if}
        </div>
      {/if}
    {/each}
    {#if rows.length === 0}
      <div class="sessions-empty" data-testid="psop-empty">
        {error ? `Query failed: ${error}` : q ? "No matching records" : "No records"}
      </div>
    {/if}
  </div>

  <div class="psop-foot">
    <div class="psop-pager">
      <button
        class="btn"
        data-testid="psop-prev"
        disabled={page <= 1}
        onclick={() => (page = Math.max(1, page - 1))}
      >
        ‹ Prev
      </button>
      {#each pageWindow() as n (n)}
        <button
          class="btn psop-pagebtn{n === page ? " btn-primary" : ""}"
          data-testid="psop-page-{n}"
          onclick={() => (page = n)}
        >
          {n}
        </button>
      {/each}
      <button
        class="btn"
        data-testid="psop-next"
        disabled={page >= pages}
        onclick={() => (page = Math.min(pages, page + 1))}
      >
        Next ›
      </button>
    </div>
    <span class="psop-count" data-testid="psop-count">
      {PAGE_SIZE} / page · total {total.toLocaleString("en-US")} · newest first (ts DESC)
    </span>
  </div>
  <div class="psop-note">
    row click expands full-text pre-render: COMMAND / STDOUT / STDERR · orange "…" =
    width-truncated, hover for tooltip
  </div>
</div>
