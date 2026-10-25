# Pi Supervisor

Windows host daemon for the **pi** coding-agent sandbox (`pi-ai`). One tray
app replaces the three legacy startup scripts (Pageant bootstrap, TCP agent
bridge, notify server) and adds sandbox lifecycle control.

## What it does

| Area | Detail |
| --- | --- |
| **Pageant SSH Agent** | Launches Pageant with every top-level `~/.ssh/*.ppk`, publishes `SSH_AUTH_SOCK` (user scope) pointing at Pageant's named pipe. |
| **Agent TCP Bridge** | `tcp://0.0.0.0:60022` — token-authenticated bridge that forwards OpenSSH agent frames to Pageant via the documented `WM_COPYDATA` protocol, so sandboxes can sign SSH ops without key files. Token: `~/.ssh/agent-bridge-token`, re-read per connection. |
| **Notify Hub** | `http://0.0.0.0:60000/notify` — drop-in replacement for the legacy node `server.js` (same wire contract); renders Windows toasts (reminder scenario). |
| **Sandbox** | Live `sbx inspect` state, one-click **Recreate** (`rm --force` → latest zot kit → `sbx run -d` → wait ready → push tokens). Detached runs are permanent — no keep-alive needed. |
| **Sessions** | 3 s poll of `pi-dtach -l` inside the sandbox; Attach opens a real terminal (`wt.exe`, falls back to `cmd /k`). |
| **Tokens** | `agent-bridge-token` + `pi-hub-token` pushed into the sandbox over `sbx exec` **stdin only** (never argv, never logged), installed `0600`. |

Service start/stop intent and window geometry persist in `config.json`.
The autostart switch is **not** stored there — it is read live from
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (`PiSupervisor` value).

Closing the window hides to the tray; **Exit** is only in the tray menu.

## Legacy cleanup

The Startup row disables the old scripts (rename to `*.disabled`):
`pageant-bootstrap.cmd`, `server.vbs` in the user Startup folder. Do not run
both stacks at once — the bridge/hub ports (60022/60000) are single-instance
by bind.

## Development

```bash
npm install
npm run dev            # vite + tauri dev window
npx svelte-check       # type/contract gate
npm run test:e2e       # Playwright mock E2E (UI contract)
cd src-tauri && cargo test   # parser/protocol unit tests
```

Networking on this LAN: cargo needs
`HTTP(S)_PROXY=http://192.168.66.7:8118`, and `zot.byte.art` must stay in
`NO_PROXY` (the app injects that for `sbx`/`reg`/`taskkill` children).

## Layout

- `src/` — Svelte 5 UI (Aqua theme, tabbed panels, mock harness on `window.__mock`)
- `src-tauri/src/` — `supervisor` (lifecycle+watchdog), `services/{pageant,bridge,notify}`,
  `sandbox` (probe/recreate), `sessions`, `tokens`, `zot`, `config`, `util`
- `pi-supervisor-plan.md` + `plan-assets/` — design plan & mockups
