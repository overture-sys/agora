<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import "@fontsource/marcellus-sc/400.css";

  type Status = { connected: boolean; model: string | null; rows: number; columns: number; message: string };
  type Input = { kind: "key" | "button" | "unknown"; index: number; pressed: boolean; raw_id: number };
  type Action = { plugin: string; plugin_name: string; category: string; action: string; name: string; tooltip: string; icon: string | null; group_icon: string | null };
  type Plugin = { uuid: string; name: string; version: string; source: string; kind: string; status: string; dir: string; hidden: boolean };
  type KeyInfo = { key: number; plugin: string; plugin_name: string; action: string; name: string; settings_url: string | null };
  type Preview = { key: number; image: string | null };
  type Line = { plugin: string; message: string };
  type Settings = { brightness: number; ring_color: string; ring_on: boolean; ring_brightness: number; use_vsd_craft_plugins: boolean };
  type LogLine = { time: string; text: string; err: boolean };
  type Page = "home" | "keys" | "plugins" | "dock";

  const pages: { id: Page; label: string }[] = [
    { id: "home", label: "Home" },
    { id: "keys", label: "Keys" },
    { id: "plugins", label: "Plugins" },
    { id: "dock", label: "Dock" },
  ];
  const ringSwatches = [
    { name: "Terracotta", color: "#d9794a" },
    { name: "Bronze", color: "#e8bc62" },
    { name: "Olive", color: "#9db868" },
    { name: "Egyptian blue", color: "#2c5a88" },
    { name: "Cinnabar", color: "#e86a52" },
    { name: "Tyrian purple", color: "#7a2c6e" },
    { name: "Marble", color: "#f3ecdf" },
  ];
  const buttonNames = ["Left", "Middle", "Right"];

  let page = $state<Page>("home");
  let status = $state<Status>({ connected: false, model: null, rows: 3, columns: 5, message: "Looking for your dock..." });
  let pressedKeys = $state(new Set<number>());
  let pressedButtons = $state(new Set<number>());
  let previews = $state<Record<number, string>>({});
  let actions = $state<Action[]>([]);
  let plugins = $state<Plugin[]>([]);
  // The key being edited; null shows the Keys overview.
  let selected = $state<number | null>(null);
  let keyInfos = $state<Record<number, KeyInfo>>({});
  let info = $state<KeyInfo | null>(null);
  let picking = $state(false);
  let search = $state("");
  let log = $state<LogLine[]>([]);
  let settings = $state<Settings>({ brightness: 80, ring_color: "#d9794a", ring_on: false, ring_brightness: 100, use_vsd_craft_plugins: false });
  let error = $state("");
  let notice = $state("");
  // The plugin whose Delete was clicked once and now asks to be sure.
  let confirming = $state<string | null>(null);

  // Plugin groups the person has opened in the action list.
  let openGroups = $state(new Set<string>());
  try {
    const saved = JSON.parse(localStorage.getItem("agora.openGroups") ?? "[]");
    if (Array.isArray(saved)) openGroups = new Set(saved);
  } catch {}

  function toggleGroup(g: string) {
    const next = new Set(openGroups);
    next.has(g) ? next.delete(g) : next.add(g);
    openGroups = next;
    try {
      localStorage.setItem("agora.openGroups", JSON.stringify([...next]));
    } catch {}
  }

  function addLog(line: string) {
    const time = new Date().toLocaleTimeString([], { hour12: false });
    const err = /error|crash|couldn't|failed|gave up/i.test(line);
    log = [{ time, text: line, err }, ...log].slice(0, 200);
  }

  async function call<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T | undefined> {
    error = "";
    notice = "";
    try {
      return await invoke<T>(cmd, args);
    } catch (e) {
      const msg = String(e);
      // Saved for later rather than failed.
      if (msg.startsWith("Saved.")) notice = msg;
      else error = msg;
    }
  }

  // ---- keys ----

  async function loadKey(k: number | null) {
    selected = k;
    search = "";
    if (k === null) {
      info = null;
      picking = false;
      return;
    }
    info = (await call<KeyInfo | null>("key_info", { key: k })) ?? null;
    picking = info === null;
  }

  function openKey(k: number) {
    page = "keys";
    loadKey(k);
  }

  function clickKey(k: number) {
    // Clicking the open key again goes back to the overview.
    loadKey(selected === k ? null : k);
  }

  async function loadOverview() {
    const list = await Promise.all(
      Array.from({ length: keyCount }, (_, k) => invoke<KeyInfo | null>("key_info", { key: k }).catch(() => null)),
    );
    const next: Record<number, KeyInfo> = {};
    list.forEach((ki, k) => ki && (next[k] = ki));
    keyInfos = next;
  }

  async function place(k: number, a: Action) {
    await call("place_action", { key: k, plugin: a.plugin, action: a.action });
    await loadKey(k);
    loadOverview();
    refreshPlugins();
  }

  /// From the overview: puts an action on the first free key.
  async function placeOnFreeKey(a: Action) {
    const free = Array.from({ length: keyCount }, (_, k) => k).find((k) => !keyInfos[k]);
    if (free === undefined) {
      error = "Every key is in use. Drag the action onto a key to replace it.";
      return;
    }
    await place(free, a);
  }

  async function removeKey() {
    if (selected === null) return;
    await call("clear_key", { key: selected });
    await loadKey(selected);
    loadOverview();
    setTimeout(refreshPlugins, 500);
  }

  // What's being dragged: an action from the list, or a key that has one.
  let dragged: Action | null = null;
  let draggedKey: number | null = null;
  let dropTarget = $state<number | null>(null);

  async function dropOn(k: number) {
    dropTarget = null;
    if (dragged) {
      await place(k, dragged);
    } else if (draggedKey !== null && draggedKey !== k) {
      const from = draggedKey;
      await call("move_key", { from, to: k });
      // Keep the moved action selected.
      await loadKey(selected === from ? k : selected === k ? from : selected);
      loadOverview();
    }
    dragged = null;
    draggedKey = null;
  }

  const groups = $derived.by(() => {
    const q = search.trim().toLowerCase();
    const out = new Map<string, Action[]>();
    for (const a of actions) {
      if (q && !`${a.name} ${a.plugin_name} ${a.tooltip}`.toLowerCase().includes(q)) continue;
      const g = a.category || a.plugin_name;
      out.set(g, [...(out.get(g) ?? []), a]);
    }
    return [...out.entries()];
  });

  // ---- plugins ----

  async function refreshPlugins() {
    plugins = (await call<Plugin[]>("list_plugins")) ?? [];
  }

  async function refreshAll() {
    actions = (await call<Action[]>("list_actions")) ?? [];
    await refreshPlugins();
    await loadKey(selected);
    await loadOverview();
  }

  async function removePlugin(p: Plugin) {
    if (confirming !== p.uuid) {
      confirming = p.uuid;
      return;
    }
    confirming = null;
    await call("remove_plugin", { uuid: p.uuid });
    await refreshAll();
  }

  async function showPlugin(p: Plugin) {
    await call("show_plugin", { uuid: p.uuid });
    await refreshAll();
  }

  async function setVsdPlugins(on: boolean) {
    settings.use_vsd_craft_plugins = on;
    await call("set_use_vsd_craft_plugins", { on });
  }

  // ---- dock ----

  async function setRing(color: string) {
    settings.ring_color = color;
    settings.ring_on = true;
    await call("set_ring", { color });
  }

  async function toggleRing() {
    if (settings.ring_on) {
      settings.ring_on = false;
      await call("ring_off");
    } else {
      await setRing(settings.ring_color);
    }
  }

  // 22 ring LEDs around the slab plus the two on the front, for the preview.
  const ringLeds = Array.from({ length: 22 }, (_, i) => {
    const t = (i / 22) * Math.PI * 2 + Math.PI;
    return { x: 110 + 96 * Math.cos(t), y: 70 + 56 * Math.sin(-t) };
  });

  // ---- status ----

  async function refreshStatus() {
    const s = await invoke<Status>("dock_status").catch(() => null);
    if (s) status = { ...s, message: s.message || status.message };
  }

  function onInput(i: Input) {
    if (i.kind === "unknown") return;
    const set = i.kind === "button" ? pressedButtons : pressedKeys;
    const next = new Set(set);
    i.pressed ? next.add(i.index) : next.delete(i.index);
    if (i.kind === "button") pressedButtons = next;
    else pressedKeys = next;
  }

  async function loadPreviews() {
    const list = (await invoke<Preview[]>("key_previews").catch(() => [])) ?? [];
    const p: Record<number, string> = {};
    for (const x of list) if (x.image) p[x.key] = x.image;
    previews = p;
  }

  const keyCount = $derived(status.rows * status.columns);
  const usedKeys = $derived(Object.keys(previews).length);
  const runningPlugins = $derived(plugins.filter((p) => p.status === "running").length);
  const visiblePlugins = $derived(plugins.filter((p) => !p.hidden).length);

  onMount(() => {
    refreshStatus();
    loadPreviews();
    call<Settings>("get_settings").then((s) => s && (settings = s));
    call<Action[]>("list_actions").then((a) => (actions = a ?? []));
    refreshPlugins();
    loadOverview();
    const onKeydown = (e: KeyboardEvent) => {
      if (e.key === "Escape" && page === "keys") loadKey(null);
    };
    window.addEventListener("keydown", onKeydown);
    // The status also comes as events; this catches any the window missed
    // while it was hidden in the tray.
    const poll = setInterval(refreshStatus, 2000);
    const onVisible = () => {
      if (!document.hidden) {
        refreshStatus();
        loadPreviews();
      }
    };
    document.addEventListener("visibilitychange", onVisible);
    window.addEventListener("focus", onVisible);
    const offs = [
      listen<Status>("dock-status", (e) => (status = e.payload)),
      listen<Input>("dock-input", (e) => onInput(e.payload)),
      listen<string>("dock-error", (e) => (error = e.payload)),
      listen<Preview>("key-preview", (e) => {
        const { key, image } = e.payload;
        const next = { ...previews };
        if (image) next[key] = image;
        else delete next[key];
        previews = next;
      }),
      listen<Line>("plugin-status", (e) => {
        addLog(`${e.payload.plugin}: ${e.payload.message}`);
        refreshPlugins();
      }),
      listen<Line>("plugin-log", (e) => addLog(`${e.payload.plugin}: ${e.payload.message}`)),
      listen("host-restarted", () => {
        loadPreviews();
        refreshAll();
      }),
    ];
    return () => {
      clearInterval(poll);
      document.removeEventListener("visibilitychange", onVisible);
      window.removeEventListener("focus", onVisible);
      window.removeEventListener("keydown", onKeydown);
      offs.forEach((p) => p.then((off) => off()));
    };
  });
</script>

{#snippet keyGrid(editable: boolean)}
  <div class="grid" style="grid-template-columns: repeat({status.columns}, 1fr)">
    {#each Array(keyCount) as _, k}
      <button
        class="key"
        class:pressed={pressedKeys.has(k)}
        class:selected={editable && selected === k}
        class:drop={dropTarget === k}
        onclick={() => (editable ? clickKey(k) : openKey(k))}
        draggable={editable && previews[k] ? "true" : "false"}
        ondragstart={(e) => {
          draggedKey = k;
          e.dataTransfer?.setData("text/plain", `key:${k}`);
          if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
        }}
        ondragend={() => {
          draggedKey = null;
          dropTarget = null;
        }}
        ondragover={(e) => {
          if (!editable || (!dragged && draggedKey === null)) return;
          e.preventDefault();
          dropTarget = k;
        }}
        ondragleave={() => {
          if (dropTarget === k) dropTarget = null;
        }}
        ondrop={(e) => {
          e.preventDefault();
          dropOn(k);
        }}
        title={editable ? (previews[k] ? `Key ${k + 1}: drag to move it` : `Key ${k + 1}`) : `Edit key ${k + 1}`}
      >
        {#if previews[k]}<img src={previews[k]} alt="" draggable="false" />{:else}<span class="num">{k + 1}</span>
          <svg class="palm" viewBox="0 0 40 40" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"
            ><path d="M20 34V22" /><path
              d="M20 22c0-8 0-14 0-18M20 22c-2-6-5-11-9-14M20 22c2-6 5-11 9-14M20 22c-5-3-9-6-14-6M20 22c5-3 9-6 14-6"
            /><path d="M12 34c2-3 5-4 8-4s6 1 8 4" /></svg
          >{/if}
      </button>
    {/each}
  </div>
  <div class="buttons">
    {#each buttonNames as name, b}
      <div class="round" class:pressed={pressedButtons.has(b)} title="{name} button"></div>
    {/each}
  </div>
{/snippet}

{#snippet library()}
    <input class="search" placeholder="Search actions" bind:value={search} />
                <div class="actions">
      {#if actions.length === 0}
        <p class="muted">No actions yet. Add plugins on the Plugins page.</p>
      {/if}
      {#each groups as [group, list]}
        {@const open = openGroups.has(group) || search.trim() !== ""}
        <button class="group" class:open onclick={() => toggleGroup(group)} aria-expanded={open}>
          <span class="chevron">▸</span>
          {#if list[0].group_icon}<img src={list[0].group_icon} alt="" />{:else}<span class="noicon"></span>{/if}
          <span class="group-name">{group}</span>
          <span class="count">{list.length}</span>
        </button>
        {#if open}
          <div class="group-items">
            {#each list as a}
              <button
                class="action"
                draggable="true"
                ondragstart={(e) => {
                  dragged = a;
                  e.dataTransfer?.setData("text/plain", `action:${a.action}`);
                }}
                ondragend={() => {
                  dragged = null;
                  dropTarget = null;
                }}
                onclick={() => (selected === null ? placeOnFreeKey(a) : place(selected, a))}
                title={a.tooltip}
              >
                {#if a.icon}<img src={a.icon} alt="" />{:else}<span class="noicon"></span>{/if}
                <span>{a.name}</span>
              </button>
            {/each}
          </div>
        {/if}
      {/each}
    </div>
{/snippet}

{#snippet activity(limit: number)}
  {#if log.length === 0}<p class="muted">Plugin messages show up here.</p>{/if}
  <div class="lines">
    {#each log.slice(0, limit) as line}<div class="ostrakon" class:err={line.err}><time>{line.time}</time>{line.text}</div>{/each}
  </div>
{/snippet}

<div class="shell">
  <nav class="rail" aria-label="Pages">
    <div class="brand">Agora</div>
    {#each pages as p}
      <button class="nav" class:active={page === p.id} onclick={() => (page = p.id)} aria-current={page === p.id ? "page" : undefined}>
        <span class="nav-icon" aria-hidden="true">
          {#if p.id === "home"}<svg viewBox="0 0 24 24"><path d="M3 10 12 4l9 6M5 10v9h14v-9M9 19v-5h6v5" /></svg>
          {:else if p.id === "keys"}<svg viewBox="0 0 24 24"><rect x="3" y="5" width="5" height="5" rx="1" /><rect x="9.5" y="5" width="5" height="5" rx="1" /><rect x="16" y="5" width="5" height="5" rx="1" /><rect x="3" y="13" width="5" height="5" rx="1" /><rect x="9.5" y="13" width="5" height="5" rx="1" /><rect x="16" y="13" width="5" height="5" rx="1" /></svg>
          {:else if p.id === "plugins"}<svg viewBox="0 0 24 24"><path d="M9 3v4M15 3v4M6 7h12v4a6 6 0 0 1-12 0zM12 17v4" /></svg>
          {:else}<svg viewBox="0 0 24 24"><ellipse cx="12" cy="12" rx="9" ry="6" /><circle cx="12" cy="12" r="2" /></svg>{/if}
        </span>
        {p.label}
      </button>
    {/each}
    <div class="rail-foot">
      <span class="pill" class:off={!status.connected}>{status.connected ? status.model ?? "Connected" : "No dock"}</span>
    </div>
  </nav>

  <main class="page-{page}">
    {#if page === "home"}
      <section class="hero">
        <div class="meander"></div>
        <div class="hero-body">
          <div>
            <h1>Agora</h1>
            <p class="tagline">The marketplace for your Stream Dock.</p>
            <span class="pill big" class:off={!status.connected}>{status.message || (status.connected ? "Connected" : "Not connected")}</span>
            {#if !status.connected}
              <p class="muted hint">Agora keeps looking and connects by itself as soon as the dock is free.</p>
            {/if}
          </div>
          <div class="hero-dock">
            {@render keyGrid(false)}
          </div>
        </div>
      </section>

      <div class="cards">
        <button class="card" onclick={() => (page = "keys")}>
          <span class="card-num">{usedKeys}<small>/{keyCount}</small></span>
          <span class="card-label">Keys set up</span>
          <span class="card-go">Edit keys</span>
        </button>
        <button class="card" onclick={() => (page = "plugins")}>
          <span class="card-num">{runningPlugins}<small>/{visiblePlugins}</small></span>
          <span class="card-label">Plugins running</span>
          <span class="card-go">Manage plugins</span>
        </button>
        <button class="card" onclick={() => (page = "dock")}>
          <span class="card-num">
            <span class="swatch" style="background: {settings.ring_on ? settings.ring_color : 'var(--line)'}"></span>
            {settings.brightness}<small>%</small>
          </span>
          <span class="card-label">Screen brightness · ring {settings.ring_on ? "on" : "off"}</span>
          <span class="card-go">Dock settings</span>
        </button>
      </div>

      <section class="log">
        <h2>Activity</h2>
        {@render activity(12)}
      </section>
    {:else if page === "keys"}
      <div class="keys-page">
        <section class="device">
          <div class="meander"></div>
          {@render keyGrid(true)}
          {#if error}<p class="error">{error}</p>{/if}
        </section>

        <section class="side">
          <div class="triglyph"></div>
          {#if selected === null}
            <div class="side-head">
              <div>
                <h2>Your keys</h2>
                <div class="muted">{Object.keys(keyInfos).length} of {keyCount} set up. Click a key to edit it.</div>
              </div>
            </div>
            {#if Object.keys(keyInfos).length > 0}
              <div class="key-list">
                {#each Object.values(keyInfos) as ki (ki.key)}
                  <button class="key-row" onclick={() => loadKey(ki.key)}>
                    {#if previews[ki.key]}<img src={previews[ki.key]} alt="" />{:else}<span class="noicon"></span>{/if}
                    <span class="key-row-text"><b><span class="k">Key {ki.key + 1}</span> · {ki.name}</b><span class="muted small">{ki.plugin_name}</span></span>
                  </button>
                {/each}
              </div>
            {/if}
            <h2 class="lib-head">Action library</h2>
            <p class="muted hint">Drag an action onto a key, or click it to use the first free key. Drag keys onto each other to move or swap them.</p>
            {@render library()}
          {:else if picking}
            <div class="side-head">
              <h2>Pick an action for key <span class="k">{selected + 1}</span></h2>
              <div class="row">
                {#if info}<button onclick={() => (picking = false)}>Cancel</button>{/if}
                <button onclick={() => loadKey(null)}>All keys</button>
              </div>
            </div>
            <p class="muted hint">Click an action to put it on key {(selected ?? 0) + 1}, or drag it onto any key.</p>
            {@render library()}
          {:else if info}
            <div class="side-head">
              <div>
                <h2><span class="k">Key {(selected ?? 0) + 1}</span> · {info.name}</h2>
                <div class="muted">{info.plugin_name}</div>
              </div>
              <div class="row">
                <button class="primary" onclick={() => (picking = true)}>Change</button>
                <button class="danger" onclick={removeKey}>Remove</button>
                <button onclick={() => loadKey(null)} title="Back to the overview (Esc)">All keys</button>
              </div>
            </div>
            {#if info.settings_url}
              {#key info.settings_url}
                <iframe class="pi" src={info.settings_url} title="Settings for {info.name}"></iframe>
              {/key}
            {:else}
              <p class="muted">This action has no settings.</p>
            {/if}
          {/if}
        </section>
      </div>
    {:else if page === "plugins"}
      <section class="plugins">
        <div class="side-head">
          <h2>Plugins</h2>
          <button onclick={() => call("open_plugins_folder")}>Open plugins folder</button>
        </div>
        <label class="switch-row">
          <input type="checkbox" role="switch" checked={settings.use_vsd_craft_plugins} onchange={(e) => setVsdPlugins(e.currentTarget.checked)} />
          <span>
            Also use plugins installed for VSD Craft
            <span class="muted small">Read only. Off while Agora is being built and tested.</span>
          </span>
        </label>
        {#if error}<p class="error">{error}</p>{/if}
        {#if plugins.length === 0}
          <p class="horos"><b>ΗΟΡΟΣ ΕΙΜΙ ΤΕΣ ΑΓΟΡΑΣ</b>"I am the boundary of the Agora." No plugins yet: put plugin folders (name.sdPlugin) in Agora's plugins folder.</p>
        {/if}
        {#each plugins as p}
          <div class="plugin" class:hidden={p.hidden}>
            <div class="plugin-text">
              <span>{p.name} <span class="muted">{p.version}</span></span>
              <span class="muted small">
                <span class="tag" class:agora={p.source === "Agora"}>{p.source === "Agora" ? "Agora" : "from VSD Craft"}</span>
                {p.kind} · {p.hidden ? "hidden" : p.status}
              </span>
            </div>
            <div class="row">
              <button class="small" onclick={() => call("open_plugin_folder", { uuid: p.uuid })}>Open folder</button>
              {#if p.hidden}
                <button class="small" onclick={() => showPlugin(p)}>Show</button>
              {:else if p.source === "Agora"}
                <button class="small danger" class:sure={confirming === p.uuid} onclick={() => removePlugin(p)}
                  title="Moves the plugin to the Recycle Bin and clears its keys"
                  >{confirming === p.uuid ? "Sure? Delete" : "Delete"}</button
                >
              {:else}
                <button class="small" class:sure={confirming === p.uuid} onclick={() => removePlugin(p)}
                  title="VSD Craft's plugins are never deleted by Agora. Hiding takes it out of the action list and clears its keys."
                  >{confirming === p.uuid ? "Sure? Hide" : "Hide"}</button
                >
              {/if}
            </div>
          </div>
        {/each}
      </section>
      <section class="log">
        <h2>Activity</h2>
        {@render activity(200)}
      </section>
    {:else}
      <div class="dock-page">
        <section class="screen-card">
          <h2>Screen</h2>
          <label class="slider">
            <span>Key brightness <b>{settings.brightness}%</b></span>
            <input
              type="range"
              min="0"
              max="100"
              bind:value={settings.brightness}
              style="--fill: {settings.brightness}%"
              onchange={() => call("set_brightness", { percent: settings.brightness })}
            />
          </label>
        </section>

        <section class="ring-card">
          <div class="side-head">
            <h2>Light ring</h2>
            <label class="switch-row tight">
              <input type="checkbox" role="switch" checked={settings.ring_on} onchange={toggleRing} />
              <span>{settings.ring_on ? "On" : "Off"}</span>
            </label>
          </div>
          <svg class="ring-preview" viewBox="0 0 220 150" role="img" aria-label="Light ring preview">
            <rect x="34" y="26" width="152" height="88" rx="14" fill="var(--key)" />
            {#each ringLeds as l}
              <circle cx={l.x} cy={l.y} r="5" fill={settings.ring_on ? settings.ring_color : "var(--line)"}
                opacity={settings.ring_on ? 0.35 + (0.65 * settings.ring_brightness) / 100 : 1} />
            {/each}
            {#each [96, 124] as x}
              <rect x={x - 6} y="132" width="12" height="4" rx="2" fill={settings.ring_on ? settings.ring_color : "var(--line)"} />
            {/each}
          </svg>
          <div class="swatches" role="group" aria-label="Ring colour">
            {#each ringSwatches as sw}
              <button
                class="swatch-btn"
                class:chosen={settings.ring_on && settings.ring_color.toLowerCase() === sw.color}
                style="--sw: {sw.color}"
                title={sw.name}
                aria-label={sw.name}
                onclick={() => setRing(sw.color)}
              ></button>
            {/each}
            <label class="swatch-btn custom" title="Pick any colour">
              <input type="color" value={settings.ring_color} onchange={(e) => setRing(e.currentTarget.value)} aria-label="Pick any colour" />
            </label>
          </div>
          <label class="slider">
            <span>Ring brightness <b>{settings.ring_brightness}%</b></span>
            <input
              type="range"
              min="0"
              max="100"
              bind:value={settings.ring_brightness}
              style="--fill: {settings.ring_brightness}%"
              onchange={() => call("set_ring_brightness", { percent: settings.ring_brightness })}
            />
          </label>
          {#if notice}<p class="muted small">{notice}</p>{/if}
          {#if error}<p class="error">{error}</p>{/if}
        </section>
      </div>
    {/if}
  </main>
</div>
