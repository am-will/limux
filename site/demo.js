// Interactive miniature of the Limux window. Plain DOM, no dependencies.
// Colors and metrics mirror the app's GTK CSS; shortcuts mirror its defaults.
(() => {
  "use strict";
  const root = document.getElementById("demo");
  const tools = document.getElementById("demo-tools");
  if (!root || !tools) return;

  const live = document.getElementById("demo-live");
  const unreadButton = tools.querySelector('[data-act="next-unread"]');
  const still = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const MAX_PANES = 6;
  const MAX_WORKSPACES = 9;
  const MAX_LINES = 120;

  const ICON = {
    sidebar: '<svg viewBox="0 0 16 16"><rect x="1.5" y="2.5" width="13" height="11" rx="1.5"/><path d="M6 2.5v11"/></svg>',
    plus: '<svg viewBox="0 0 16 16"><path d="M8 3v10M3 8h10"/></svg>',
    term: '<svg viewBox="0 0 16 16"><rect x="1.5" y="2.5" width="13" height="11" rx="1.5"/><path d="M4 6l2.5 2L4 10M8 10.5h3"/></svg>',
    right: '<svg viewBox="0 0 16 16"><rect x="1.5" y="2.5" width="13" height="11" rx="1.5"/><path d="M8 2.5v11"/></svg>',
    down: '<svg viewBox="0 0 16 16"><rect x="1.5" y="2.5" width="13" height="11" rx="1.5"/><path d="M1.5 8h13"/></svg>',
    close: '<svg viewBox="0 0 16 16"><path d="M4 4l8 8M12 4l-8 8"/></svg>',
    chevron: '<svg viewBox="0 0 10 10"><path d="M1 3h8L5 8z"/></svg>',
    min: '<svg viewBox="0 0 12 12"><path d="M2 10h8"/></svg>',
    max: '<svg viewBox="0 0 12 12"><rect x="2" y="2" width="8" height="8"/></svg>',
    x: '<svg viewBox="0 0 12 12"><path d="M2 2l8 8M10 2l-8 8"/></svg>',
  };

  const esc = (text) =>
    String(text).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const span = (cls, text) => `<span class="${cls}">${esc(text)}</span>`;

  let seq = 0;
  let state;
  let timers = [];
  let toastTimer = 0;
  let visible = false;

  const later = (ms, fn) => timers.push(setTimeout(fn, ms));

  function makeTab(title, lines, extra) {
    return Object.assign({ id: ++seq, title, lines: lines.slice(), input: "", mode: "shell", unread: false, busy: false, exit: null }, extra);
  }
  function makePane(...tabs) {
    return { id: ++seq, tabs, active: 0 };
  }
  function makeWorkspace(name, path, group, tree) {
    const ws = { id: ++seq, name, path, group, tree, message: "", zoom: null, focus: null };
    ws.focus = panesOf(tree)[0].id;
    return ws;
  }
  function panesOf(node) {
    return node.kids ? node.kids.flatMap(panesOf) : [node];
  }

  function initialState() {
    const api = makeWorkspace("shop-api", "~/code/shop-api", "shop", {
      dir: "row",
      kids: [
        makePane(
          makeTab("agent", [
            span("m", "> rate limit the login route"),
            "",
            span("b", "read ") + " src/routes/login.ts",
            span("y", "edit ") + " src/middleware/limit.ts  " + span("g", "+38") + " " + span("r", "-2"),
            span("b", "run  ") + " npm test",
          ], { mode: "busy", busy: true }),
        ),
        {
          dir: "col",
          kids: [
            makePane(
              makeTab("dev server", [
                span("m", "$ ") + "npm run dev",
                span("g", "ready") + " on http://localhost:3000",
                span("m", "GET  /health  200  2ms"),
              ], { mode: "busy" }),
            ),
            makePane(makeTab("shell", [span("m", "Type ") + span("w", "help") + span("m", " to see what works here.")])),
          ],
        },
      ],
    });
    const web = makeWorkspace("shop-web", "~/code/shop-web", "shop",
      makePane(makeTab("agent", [
        span("m", "> add order history to the account page"),
        "",
        span("b", "read ") + " db/schema.sql",
        span("y", "edit ") + " app/account/orders.tsx  " + span("g", "+112"),
        span("y", "edit ") + " db/migrations/0042_orders.sql  " + span("g", "+19"),
      ], { mode: "busy", busy: true })));
    const infra = makeWorkspace("infra", "~/code/infra", null,
      makePane(makeTab("plan", [
        span("m", "$ ") + "terraform plan",
        "Refreshing state...",
      ], { mode: "busy", busy: true })));
    return {
      groups: { shop: { name: "shop", color: "#1d4ed8", shut: false } },
      workspaces: [api, web, infra],
      active: api.id,
      sidebar: true,
      toast: "",
    };
  }

  const activeWs = () => state.workspaces.find((w) => w.id === state.active);
  const focusedPane = () => {
    const ws = activeWs();
    return panesOf(ws.tree).find((p) => p.id === ws.focus) || panesOf(ws.tree)[0];
  };
  const focusedTab = () => {
    const pane = focusedPane();
    return pane.tabs[pane.active];
  };
  const wsUnread = (ws) => panesOf(ws.tree).some((p) => p.tabs.some((t) => t.unread));
  const everyTab = () =>
    state.workspaces.flatMap((ws) => panesOf(ws.tree).flatMap((pane) => pane.tabs.map((tab, index) => ({ ws, pane, tab, index }))));
  const locate = (tab) => everyTab().find((entry) => entry.tab === tab);

  function say(text) {
    if (live) live.textContent = text;
  }
  function print(tab, ...lines) {
    tab.lines.push(...lines);
    if (tab.lines.length > MAX_LINES) tab.lines.splice(0, tab.lines.length - MAX_LINES);
  }
  function toast(text) {
    state.toast = text;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { state.toast = ""; render(); }, 3200);
  }
  function markRead() {
    focusedTab().unread = false;
    const ws = activeWs();
    if (!wsUnread(ws)) ws.message = "";
  }
  function raise(tab, message) {
    const at = locate(tab);
    if (!at) return;
    tab.busy = false;
    const watching = visible && at.ws.id === state.active && at.pane.id === at.ws.focus && at.pane.tabs[at.pane.active] === tab;
    if (!watching) {
      tab.unread = true;
      at.ws.message = message;
    }
    toast(`${at.ws.name}: ${message}`);
    say(`${at.ws.name}: ${message}`);
    render();
  }

  // ---- layout ---------------------------------------------------------
  function parentOf(node, id) {
    if (!node.kids) return null;
    for (const kid of node.kids) {
      if (!kid.kids && kid.id === id) return node;
      const found = parentOf(kid, id);
      if (found) return found;
    }
    return null;
  }
  function split(direction, tab) {
    const ws = activeWs();
    if (panesOf(ws.tree).length >= MAX_PANES) {
      toast("The demo stops at six panes. Limux does not.");
      return null;
    }
    ws.zoom = null;
    const dir = direction === "left" || direction === "right" ? "row" : "col";
    const before = direction === "left" || direction === "up";
    const old = focusedPane();
    const pane = makePane(tab || makeTab("shell", []));
    const parent = parentOf(ws.tree, old.id);
    if (parent && parent.dir === dir) {
      parent.kids.splice(parent.kids.indexOf(old) + (before ? 0 : 1), 0, pane);
    } else {
      const node = { dir, kids: before ? [pane, old] : [old, pane] };
      if (parent) parent.kids[parent.kids.indexOf(old)] = node;
      else ws.tree = node;
    }
    ws.focus = pane.id;
    return pane;
  }
  function closePane(pane) {
    const ws = activeWs();
    ws.zoom = null;
    const parent = parentOf(ws.tree, pane.id);
    if (!parent) {
      closeWorkspace(ws);
      return;
    }
    const at = parent.kids.indexOf(pane);
    parent.kids.splice(at, 1);
    if (parent.kids.length === 1) Object.assign(parent, collapse(parent));
    const panes = panesOf(ws.tree);
    if (!panes.some((p) => p.id === ws.focus)) ws.focus = panes[Math.min(at, panes.length - 1)].id;
    markRead();
  }
  // A split with one child becomes that child, in place.
  function collapse(node) {
    const only = node.kids[0];
    for (const key of Object.keys(node)) delete node[key];
    return only;
  }
  function closeWorkspace(ws) {
    if (state.workspaces.length === 1) {
      ws.tree = makePane(makeTab("shell", []));
      ws.focus = ws.tree.id;
      return;
    }
    const at = state.workspaces.indexOf(ws);
    state.workspaces.splice(at, 1);
    if (state.active === ws.id) state.active = state.workspaces[Math.max(0, at - 1)].id;
    markRead();
  }
  function closeTab(pane, index) {
    if (pane.tabs.length === 1) {
      closePane(pane);
      return;
    }
    pane.tabs.splice(index, 1);
    pane.active = Math.min(pane.active, pane.tabs.length - 1);
  }
  function addTab() {
    const pane = focusedPane();
    if (pane.tabs.length >= 4) return;
    pane.tabs.push(makeTab("shell", []));
    pane.active = pane.tabs.length - 1;
  }
  function addWorkspace(name, path) {
    if (state.workspaces.length >= MAX_WORKSPACES) return null;
    const ws = makeWorkspace(name || "home", path || "~", null, makePane(makeTab("shell", [])));
    state.workspaces.push(ws);
    select(ws);
    return ws;
  }
  function select(ws) {
    if (ws.group) state.groups[ws.group].shut = false;
    state.active = ws.id;
    markRead();
  }
  function nextUnread(step) {
    const all = everyTab();
    const here = all.findIndex((e) => e.tab === focusedTab());
    for (let i = 1; i <= all.length; i++) {
      const entry = all[(here + step * i + all.length * 2) % all.length];
      if (!entry.tab.unread) continue;
      entry.ws.focus = entry.pane.id;
      entry.ws.zoom = null;
      entry.pane.active = entry.index;
      select(entry.ws);
      say(`Jumped to ${entry.ws.name}`);
      return;
    }
    toast("Nothing is waiting for you");
  }
  function focusDirection(direction) {
    const ws = activeWs();
    if (ws.zoom) return;
    const here = root.querySelector(`[data-pane="${ws.focus}"]`);
    if (!here) return;
    const a = here.getBoundingClientRect();
    let best = null;
    for (const el of root.querySelectorAll("[data-pane]")) {
      if (el === here) continue;
      const b = el.getBoundingClientRect();
      const gap = { left: a.left - b.right, right: b.left - a.right, up: a.top - b.bottom, down: b.top - a.bottom }[direction];
      const overlap = direction === "left" || direction === "right"
        ? Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top)
        : Math.min(a.right, b.right) - Math.max(a.left, b.left);
      if (gap < -2 || overlap <= 0) continue;
      const score = gap * 1000 - overlap;
      if (!best || score < best.score) best = { score, id: Number(el.dataset.pane) };
    }
    if (best) ws.focus = best.id;
    markRead();
  }

  // ---- shell ----------------------------------------------------------
  const HELP = [
    "This shell is part of the demo. These work:",
    "",
    "  " + span("w", "limux new-pane --direction right|down|left|up"),
    "  " + span("w", "limux new-workspace --cwd ~/code/blog"),
    "  " + span("w", "limux list-workspaces"),
    "  " + span("w", "limux select-workspace --workspace infra"),
    "  " + span("w", "limux notify --workspace infra \"look here\""),
    "  " + span("w", "limux agent-team --agents codex,claude"),
    "  " + span("w", "ls, pwd, echo, clear, exit"),
  ];
  function words(line) {
    return (line.match(/"[^"]*"|'[^']*'|\S+/g) || []).map((w) => w.replace(/^(["'])(.*)\1$/, "$2"));
  }
  function parse(args, valued) {
    const opts = {};
    const rest = [];
    for (let i = 0; i < args.length; i++) {
      if (args[i].startsWith("--")) opts[args[i].slice(2)] = valued.includes(args[i].slice(2)) ? args[++i] || "" : true;
      else rest.push(args[i]);
    }
    return { opts, rest };
  }
  function findWorkspace(name) {
    return state.workspaces.find((w) => w.name === name || String(w.id) === name);
  }
  function limux(tab, args) {
    const command = args[0];
    const { opts, rest } = parse(args.slice(1), ["direction", "cwd", "workspace", "command", "agents", "body", "subtitle"]);
    switch (command) {
      case "new-pane": {
        const direction = opts.direction || "right";
        if (!["left", "right", "up", "down"].includes(direction)) {
          print(tab, span("r", "error: ") + "direction must be left, right, up or down");
          return;
        }
        const lines = opts.command ? [span("m", "$ ") + esc(opts.command), span("m", "(the demo does not run programs)")] : [];
        split(direction, makeTab("shell", lines));
        return;
      }
      case "new-workspace": {
        const path = opts.cwd || "~";
        const name = path.replace(/\/+$/, "").split("/").pop();
        if (!addWorkspace(name === "~" ? "home" : name, path)) print(tab, "The demo stops at nine workspaces.");
        return;
      }
      case "list-workspaces":
        for (const ws of state.workspaces) print(tab, `${ws.id === state.active ? "*" : " "} ${esc(ws.name)}  ${span("m", ws.path)}`);
        return;
      case "select-workspace": {
        const ws = findWorkspace(opts.workspace);
        if (ws) select(ws);
        else print(tab, span("r", "error: ") + "workspace not found");
        return;
      }
      case "notify": {
        const title = rest.join(" ") || "Input needed";
        const message = opts.body ? `${title}: ${opts.body}` : title;
        const ws = opts.workspace ? findWorkspace(opts.workspace) : activeWs();
        if (!ws) {
          print(tab, span("r", "error: ") + "workspace not found");
          return;
        }
        if (ws.id === state.active) toast(message);
        else raise(panesOf(ws.tree)[0].tabs[0], message);
        return;
      }
      case "agent-team": {
        const home = activeWs().focus;
        (opts.agents || "codex,claude").split(",").slice(0, 3).forEach((agent, index) => {
          split(index === 0 ? "right" : "down", makeTab(agent, [
            span("m", "$ ") + esc(agent),
            span("m", "reading AGENTS.md"),
            span("m", "(in Limux, the real agent starts here)"),
          ], { mode: "shell" }));
        });
        activeWs().focus = home;
        print(tab, "wrote AGENTS.md");
        return;
      }
      default:
        print(tab, ...HELP);
    }
  }
  function prompt(ws) {
    return span("b", ws.path) + " " + span("m", "$ ");
  }
  function run(tab, line) {
    const at = locate(tab);
    print(tab, prompt(at.ws) + esc(line));
    const [name, ...args] = words(line);
    if (!name) return;
    switch (name) {
      case "help": print(tab, ...HELP); return;
      case "limux": limux(tab, args); return;
      case "ls": print(tab, span("b", "docs  src  tests") + "  README.md"); return;
      case "pwd": print(tab, esc(at.ws.path.replace("~", "/home/you"))); return;
      case "echo": print(tab, esc(args.join(" "))); return;
      case "clear": tab.lines = []; return;
      case "exit": closeTab(at.pane, at.index); return;
      default: print(tab, esc(name) + ": not in this demo. Try " + span("w", "help"));
    }
  }
  function typeKey(event) {
    const tab = focusedTab();
    if (tab.exit !== null) {
      if (event.key === "Enter") restart(tab);
      else return false;
      return true;
    }
    if (tab.mode === "ask") {
      const key = event.key.toLowerCase();
      if (key !== "y" && key !== "n") return false;
      tab.lines[tab.lines.length - 1] += " " + esc(key);
      print(tab, "", key === "y" ? span("g", "migration applied") + " in 0.4s" : "skipped. The migration is still pending.", "");
      tab.mode = "shell";
      return true;
    }
    if (event.key === "c" && event.ctrlKey && tab.mode === "busy") {
      print(tab, "^C", "");
      tab.mode = "shell";
      tab.busy = false;
      return true;
    }
    if (tab.mode !== "shell") return false;
    if (event.key === "Enter") {
      const line = tab.input;
      tab.input = "";
      run(tab, line);
      return true;
    }
    if (event.key === "Backspace") {
      tab.input = tab.input.slice(0, -1);
      return true;
    }
    if (event.ctrlKey || event.altKey || event.metaKey) {
      if (event.ctrlKey && !event.altKey && event.key === "u") tab.input = "";
      else if (event.ctrlKey && !event.altKey && event.key === "c") { print(tab, prompt(activeWs()) + esc(tab.input) + "^C"); tab.input = ""; }
      else return false;
      return true;
    }
    if (event.key.length === 1 && tab.input.length < 200) {
      tab.input += event.key;
      return true;
    }
    return false;
  }
  function restart(tab) {
    tab.exit = null;
    tab.unread = false;
    tab.mode = "busy";
    tab.lines = [span("m", "$ ") + "terraform plan", "Refreshing state..."];
    tab.busy = true;
    later(still ? 0 : 1400, () => {
      print(tab, span("g", "No changes.") + " Infrastructure matches the configuration.", "");
      tab.busy = false;
      tab.mode = "shell";
      render();
    });
  }

  // ---- actions --------------------------------------------------------
  const ACTIONS = {
    "split-right": () => split("right"),
    "split-down": () => split("down"),
    zoom: () => { const ws = activeWs(); ws.zoom = ws.zoom || panesOf(ws.tree).length < 2 ? null : ws.focus; },
    "next-unread": () => nextUnread(1),
    "prev-unread": () => nextUnread(-1),
    sidebar: () => { state.sidebar = !state.sidebar; },
    "new-tab": addTab,
    "new-workspace": () => addWorkspace(),
    "close-pane": () => closePane(focusedPane()),
    "close-tab": () => { const pane = focusedPane(); closeTab(pane, pane.active); },
    reset: start,
  };
  function shortcut(event) {
    if (!event.ctrlKey || event.metaKey) return null;
    const code = event.code;
    if (event.altKey && !event.shiftKey) {
      if (/^Digit[1-9]$/.test(code)) return () => { const ws = state.workspaces[Number(code.slice(5)) - 1]; if (ws) select(ws); };
      const arrow = { ArrowLeft: "left", ArrowRight: "right", ArrowUp: "up", ArrowDown: "down" }[code];
      if (arrow) return () => focusDirection(arrow);
      if (code === "PageDown" || code === "PageUp") {
        return () => {
          const all = state.workspaces;
          const at = all.indexOf(activeWs()) + (code === "PageDown" ? 1 : -1);
          select(all[(at + all.length) % all.length]);
        };
      }
      return ACTIONS[{ KeyD: "split-right", KeyU: "next-unread", KeyM: "sidebar", KeyT: "new-tab", KeyW: "close-pane" }[code]];
    }
    if (event.altKey && event.shiftKey) return ACTIONS[{ KeyD: "split-down", KeyU: "prev-unread", KeyN: "new-workspace" }[code]];
    if (event.shiftKey) return ACTIONS[{ KeyZ: "zoom" }[code]];
    return null;
  }

  // ---- render ---------------------------------------------------------
  function renderTerm(ws, pane, tab) {
    const lines = tab.lines.join("\n");
    const cursor = '<i class="d-cur"></i>';
    let tail = "";
    if (tab.exit !== null) {
      tail = `\n\n<span class="r">Process exited with code ${tab.exit}</span>\n` +
        `<button type="button" tabindex="-1" class="d-exit" data-restart="${tab.id}">Restart</button> ` +
        `<button type="button" tabindex="-1" class="d-exit" data-close-tab="${pane.id}:${pane.active}">Close</button>`;
    } else if (tab.mode === "shell") {
      tail = (lines ? "\n" : "") + prompt(ws) + esc(tab.input) + cursor;
    } else if (tab.mode === "ask") {
      tail = " " + cursor;
    } else {
      tail = "\n" + cursor;
    }
    return `<div class="d-term" data-term="${tab.id}">${lines}${tail}</div>`;
  }
  function renderPane(ws, pane) {
    const tab = pane.tabs[pane.active];
    const tabs = pane.tabs.map((t, i) =>
      `<div class="d-tab${i === pane.active ? " on" : ""}" data-tab="${pane.id}:${i}">` +
      (t.unread ? '<span class="ud"></span>' : t.busy && i !== pane.active ? '<span class="sp"></span>' : "") +
      `${esc(t.title)}<span class="d-x" data-close-tab="${pane.id}:${i}">&times;</span></div>`).join("");
    const button = (act, icon, label) =>
      `<button type="button" tabindex="-1" class="d-ic" data-act="${act}" data-in="${pane.id}" title="${label}" aria-label="${label}">${ICON[icon]}</button>`;
    return `<div class="d-pane${pane.id === ws.focus ? " focus" : ""}" data-pane="${pane.id}">` +
      `<div class="d-ph">${tabs}<span class="d-grow"></span>` +
      button("new-tab", "term", "New tab") + button("split-right", "right", "Split right") +
      button("split-down", "down", "Split down") + button("close-pane", "close", "Close pane") +
      `</div>${renderTerm(ws, pane, tab)}</div>`;
  }
  function renderNode(ws, node) {
    if (!node.kids) return renderPane(ws, node);
    return `<div class="d-split${node.dir === "col" ? " col" : ""}">${node.kids.map((k) => renderNode(ws, k)).join("")}</div>`;
  }
  function renderRow(ws) {
    const group = ws.group && state.groups[ws.group];
    const unread = wsUnread(ws);
    return `<div class="d-row${ws.id === state.active ? " on" : ""}${unread ? " unread" : ""}${group ? " member" : ""}" data-ws="${ws.id}"` +
      `${group ? ` style="--g:${group.color}"` : ""}><div class="nm">${esc(ws.name)}</div><div class="pt">${esc(ws.path)}</div>` +
      (unread && ws.message ? `<div class="ms">${esc(ws.message)}</div>` : "") + "</div>";
  }
  function renderSidebar() {
    const out = [];
    const done = new Set();
    for (const ws of state.workspaces) {
      if (!ws.group) { out.push(renderRow(ws)); continue; }
      if (done.has(ws.group)) continue;
      done.add(ws.group);
      const group = state.groups[ws.group];
      const members = state.workspaces.filter((w) => w.group === ws.group);
      out.push(`<div class="d-group${group.shut ? " shut" : ""}" data-group="${ws.group}" style="--g:${group.color}">${ICON.chevron}<b>${group.name}</b>` +
        (group.shut ? `<span class="n">${members.length}</span>` : "") + "</div>");
      if (!group.shut) out.push(...members.map(renderRow));
    }
    return out.join("");
  }
  function render() {
    const ws = activeWs();
    const zoomed = ws.zoom && panesOf(ws.tree).find((p) => p.id === ws.zoom);
    root.classList.toggle("no-side", !state.sidebar);
    root.innerHTML =
      `<div class="d-top"><button type="button" tabindex="-1" class="d-ic" data-act="sidebar" title="Toggle sidebar" aria-label="Toggle sidebar">${ICON.sidebar}</button>` +
      `<button type="button" tabindex="-1" class="d-ic" data-act="new-workspace" title="New workspace" aria-label="New workspace">${ICON.plus}</button>` +
      `<div class="d-pills">${state.workspaces.map((w) => `<div class="d-pill${w.id === state.active ? " on" : ""}${wsUnread(w) ? " unread" : ""}" data-ws="${w.id}">${esc(w.name)}</div>`).join("")}</div>` +
      `<span class="d-grow"></span><div class="d-win" aria-hidden="true"><span>${ICON.min}</span><span>${ICON.max}</span><span>${ICON.x}</span></div></div>` +
      `<div class="d-main"><div class="d-side"><div class="d-rows">${renderSidebar()}</div><div class="d-ssh">Connect via SSH…</div></div>` +
      `<div class="d-content">${zoomed ? renderPane(ws, zoomed) : renderNode(ws, ws.tree)}</div></div>` +
      (state.toast ? `<div class="d-toast">${esc(state.toast)}</div>` : "");
    for (const term of root.querySelectorAll(".d-term")) term.scrollTop = term.scrollHeight;
    unreadButton.classList.toggle("hot", state.workspaces.some(wsUnread));
  }

  // ---- events ---------------------------------------------------------
  function act(name) {
    const fn = ACTIONS[name];
    if (!fn) return;
    fn();
    if (name !== "reset") markRead();
    render();
  }
  root.addEventListener("click", (event) => {
    const el = event.target.closest("[data-close-tab],[data-restart],[data-act],[data-tab],[data-group],[data-ws],[data-pane]");
    if (!el) return;
    const ws = activeWs();
    const paneById = (id) => panesOf(ws.tree).find((p) => p.id === Number(id));
    const { dataset: d } = el;
    if (d.closeTab !== undefined || d.tab !== undefined) {
      const [id, index] = (d.closeTab || d.tab).split(":");
      const pane = paneById(id);
      ws.focus = pane.id;
      if (d.closeTab !== undefined) closeTab(pane, Number(index));
      else pane.active = Number(index);
    } else if (d.restart !== undefined) {
      const entry = everyTab().find((e) => e.tab.id === Number(d.restart));
      if (entry) restart(entry.tab);
    } else if (d.act !== undefined) {
      if (d.in) ws.focus = Number(d.in);
      ACTIONS[d.act]();
    } else if (d.group !== undefined) {
      state.groups[d.group].shut = !state.groups[d.group].shut;
    } else if (d.ws !== undefined) {
      select(state.workspaces.find((w) => w.id === Number(d.ws)));
    } else if (!getSelection().toString()) {
      ws.focus = Number(d.pane);
    }
    markRead();
    render();
    root.focus({ preventScroll: true });
  });
  root.addEventListener("keydown", (event) => {
    const fn = shortcut(event);
    if (fn) {
      event.preventDefault();
      fn();
      markRead();
      render();
      return;
    }
    if (event.key === "Tab" || event.key === "Escape") return;
    if (typeKey(event)) {
      event.preventDefault();
      render();
    }
  });
  tools.addEventListener("click", (event) => {
    const button = event.target.closest("[data-act]");
    if (button) act(button.dataset.act);
  });

  // ---- story ----------------------------------------------------------
  function story() {
    const [api, web, infra] = state.workspaces;
    const agent = panesOf(api.tree)[0].tabs[0];
    const dev = panesOf(api.tree)[1].tabs[0];
    const asking = web.tree.tabs[0];
    const plan = infra.tree.tabs[0];
    const d = still ? 0 : 1;
    later(1200 * d, () => { print(dev, span("m", "POST /login   200  41ms")); render(); });
    later(2600 * d, () => { print(dev, span("m", "POST /login   ") + span("y", "429") + span("m", "   1ms")); render(); });
    later(3400 * d, () => {
      print(asking, "", "Run " + span("w", "db/migrations/0042_orders.sql") + " now? [y/n]");
      asking.mode = "ask";
      raise(asking, "Agent needs you");
    });
    later(5200 * d, () => {
      print(agent, "      42 passed, 0 failed", "", span("g", "done ") + " /login allows 5 tries a minute", "");
      agent.mode = "shell";
      agent.busy = false;
      render();
    });
    later(11000 * d, () => {
      print(plan, span("r", "Error: ") + "credentials have expired");
      plan.exit = 1;
      raise(plan, "Process exited with code 1");
    });
  }
  let told = false;
  function start() {
    timers.forEach(clearTimeout);
    timers = [];
    seq = 0;
    state = initialState();
    if (root.clientWidth < 560) activeWs().zoom = activeWs().focus;
    render();
    told = false;
    if (visible) tell();
  }
  function tell() {
    if (told) return;
    told = true;
    story();
  }

  root.tabIndex = 0;
  root.setAttribute("role", "application");
  root.setAttribute("aria-roledescription", "demo");
  tools.hidden = false;
  new IntersectionObserver((entries) => {
    visible = entries[0].isIntersecting;
    if (visible) tell();
  }, { threshold: 0.35 }).observe(root);
  start();
})();
