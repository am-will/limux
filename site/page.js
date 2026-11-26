// Tabs, copy buttons, shortcut filter, and the latest release version.
(() => {
  "use strict";

  for (const list of document.querySelectorAll('[role="tablist"]')) {
    const tabs = [...list.querySelectorAll('[role="tab"]')];
    const choose = (tab, focus) => {
      for (const other of tabs) {
        const on = other === tab;
        other.setAttribute("aria-selected", String(on));
        other.tabIndex = on ? 0 : -1;
        document.getElementById(other.getAttribute("aria-controls")).hidden = !on;
      }
      if (focus) tab.focus();
    };
    list.addEventListener("click", (event) => {
      const tab = event.target.closest('[role="tab"]');
      if (tab) choose(tab, false);
    });
    list.addEventListener("keydown", (event) => {
      const at = tabs.indexOf(document.activeElement);
      const to = { ArrowRight: at + 1, ArrowLeft: at - 1, Home: 0, End: tabs.length - 1 }[event.key];
      if (at < 0 || to === undefined) return;
      event.preventDefault();
      choose(tabs[(to + tabs.length) % tabs.length], true);
    });
  }

  if (navigator.clipboard) {
    for (const box of document.querySelectorAll(".cmd")) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "copy";
      button.textContent = "Copy";
      button.addEventListener("click", async () => {
        try {
          await navigator.clipboard.writeText(box.querySelector("code").textContent);
          button.textContent = "Copied";
        } catch {
          button.textContent = "Press Ctrl+C";
        }
        setTimeout(() => { button.textContent = "Copy"; }, 1600);
      });
      box.append(button);
    }
  }

  const filter = document.getElementById("key-filter");
  if (filter) {
    const tables = [...document.querySelectorAll("#key-cols table")];
    const none = document.getElementById("key-none");
    filter.addEventListener("input", () => {
      const needle = filter.value.trim().toLowerCase();
      let any = false;
      for (const table of tables) {
        const named = table.caption.textContent.toLowerCase().includes(needle);
        let shown = false;
        for (const row of table.rows) {
          const hit = named || row.textContent.toLowerCase().includes(needle);
          row.hidden = !hit;
          shown ||= hit;
        }
        table.hidden = !shown;
        any ||= shown;
      }
      none.hidden = any;
    });
  }

  // The page works with the version in the markup. This only refreshes it.
  fetch("https://api.github.com/repos/am-will/limux/releases/latest")
    .then((response) => (response.ok ? response.json() : null))
    .then((release) => {
      const version = /^v(\d+\.\d+\.\d+)$/.exec(release?.tag_name || "")?.[1];
      if (!version) return;
      for (const el of document.querySelectorAll("[data-version]")) el.textContent = version;
    })
    .catch(() => {});
})();
