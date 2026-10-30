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

  let status = $state<Status>({ connected: false, model: null, rows: 3, columns: 5, message: "Starting..." });
  let pressedKeys = $state(new Set<number>());
  let pressedButtons = $state(new Set<number>());
  let previews = $state<Record<number, string>>({});
  let actions = $state<Action[]>([]);
  let plugins = $state<Plugin[]>([]);
  let selected = $state<number>(0);
  let info = $state<KeyInfo | null>(null);
  let picking = $state(false);
  let search = $state("");
  // Plugin groups the person has opened in the action list.
  let openGroups = $state(new Set<string>());

  function toggleGroup(g: string) {
    const next = new Set(openGroups);
    next.has(g) ? next.delete(g) : next.add(g);
    openGroups = next;
    try {
      localStorage.setItem("agora.openGroups", JSON.stringify([...next]));
    } catch {}
  }

  try {
    const saved = JSON.parse(localStorage.getItem("agora.openGroups") ?? "[]");
    if (Array.isArray(saved)) openGroups = new Set(saved);
  } catch {}
  type LogLine = { time: string; text: string; err: boolean };
  let log = $state<LogLine[]>([]);
  let ringOn = $state(false);
  // 24 LEDs around an oval, for the ring preview.
  const leds = Array.from({ length: 24 }, (_, i) => {
    const t = (i / 24) * Math.PI * 2;
    return { x: 60 + 50 * Math.cos(t), y: 40 + 30 * Math.sin(t) };
  });
  let brightness = $state(80);
  let ringColor = $state("#d9794a");
  let error = $state("");

  const buttonNames = ["Left", "Middle", "Right"];

  function addLog(line: string) {
    const time = new Date().toLocaleTimeString([], { hour12: false });
    const err = /error|crash|couldn't|failed|gave up/i.test(line);
    log = [{ time, text: line, err }, ...log].slice(0, 200);
  }

  async function call<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T | undefined> {
    error = "";
    try {
      return await invoke<T>(cmd, args);
    } catch (e) {
      error = String(e);
    }
  }

  async function loadKey(k: number) {
    selected = k;
    info = (await call<KeyInfo | null>("key_info", { key: k })) ?? null;
    picking = info === null;
  }

  async function place(k: number, a: Action) {
    await call("place_action", { key: k, plugin: a.plugin, action: a.action });
    await loadKey(k);
    refreshPlugins();
  }

  async function removeKey() {
    await call("clear_key", { key: selected });
    await loadKey(selected);
    setTimeout(refreshPlugins, 500);
  }

  // The plugin whose Delete was clicked once and now asks to be sure.
  let confirming = $state<string | null>(null);

  async function removePlugin(p: Plugin) {
    if (confirming !== p.uuid) {
      confirming = p.uuid;
      return;
    }
    confirming = null;
    await call("remove_plugin", { uuid: p.uuid });
    await afterPluginChange();
  }

  async function showPlugin(p: Plugin) {
    await call("show_plugin", { uuid: p.uuid });
    await afterPluginChange();
  }

  async function afterPluginChange() {
    actions = (await call<Action[]>("list_actions")) ?? [];
    await refreshPlugins();
    await loadKey(selected);
  }

  async function refreshPlugins() {
    plugins = (await call<Plugin[]>("list_plugins")) ?? [];
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

  function onInput(i: Input) {
    if (i.kind === "unknown") return;
    const set = i.kind === "button" ? pressedButtons : pressedKeys;
    const next = new Set(set);
    i.pressed ? next.add(i.index) : next.delete(i.index);
    if (i.kind === "button") pressedButtons = next;
    else pressedKeys = next;
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
    }
    dragged = null;
    draggedKey = null;
  }

  onMount(() => {
    invoke<Status>("dock_status").then((s) => (status = { ...s, message: s.message || status.message }));
    invoke<Preview[]>("key_previews").then((list) => {
      const p: Record<number, string> = {};
      for (const x of list) if (x.image) p[x.key] = x.image;
      previews = { ...p, ...previews };
    });
    call<Action[]>("list_actions").then((a) => (actions = a ?? []));
    refreshPlugins();
    loadKey(0);
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
    ];
    return () => offs.forEach((p) => p.then((off) => off()));
  });
</script>

<main>
  <header>
    <div class="head-row">
      <h1>Agora</h1>
      <span class="pill" class:off={!status.connected}>{status.message || (status.connected ? "Connected" : "Not connected")}</span>
    </div>
    <div class="meander"></div>
  </header>

  <div class="left">
    <section class="device">
      <div class="meander"></div>
      <div class="grid" style="grid-template-columns: repeat({status.columns}, 1fr)">
        {#each Array(status.rows * status.columns) as _, k}
          <button
            class="key"
            class:pressed={pressedKeys.has(k)}
            class:selected={selected === k}
            class:drop={dropTarget === k}
            onclick={() => loadKey(k)}
            draggable={previews[k] ? "true" : "false"}
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
              if (!dragged && draggedKey === null) return;
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
            title={previews[k] ? `Key ${k + 1}: drag to move it` : `Key ${k + 1}`}
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
    </section>

    <section class="controls">
      <label>
        Brightness {brightness}%
        <input
          type="range"
          min="0"
          max="100"
          bind:value={brightness}
          style="--fill: {brightness}%"
          onchange={() => call("set_brightness", { percent: brightness })}
        />
      </label>
      <div class="ring">
        <svg class="orbit" viewBox="0 0 120 80" role="img" aria-label="Light ring, 24 LEDs">
          <ellipse cx="60" cy="40" rx="50" ry="30" fill="none" stroke="var(--line)" stroke-width="1" />
          {#each leds as l}<circle cx={l.x} cy={l.y} r="3.2" fill={ringOn ? ringColor : "var(--line)"} />{/each}
          <circle cx="60" cy="40" r="11" fill={ringColor} stroke="var(--key)" stroke-width="2" />
        </svg>
        <div class="ring-actions">
          <label class="ring-pick">Light ring <input type="color" bind:value={ringColor} /></label>
          <button class="primary" onclick={async () => { await call("set_ring", { color: ringColor }); ringOn = !error; }}>Set ring</button>
          <button onclick={async () => { await call("ring_off"); ringOn = false; }}>Ring off</button>
        </div>
      </div>
      {#if error}<p class="error">{error}</p>{/if}
    </section>

    <section class="plugins">
      <div class="side-head">
        <h2>Plugins</h2>
        <button onclick={() => call("open_plugins_folder")}>Open plugins folder</button>
      </div>
      {#if plugins.length === 0}
        <p class="horos"><b>ΗΟΡΟΣ ΕΙΜΙ ΤΕΣ ΑΓΟΡΑΣ</b>"I am the boundary of the Agora." No plugins yet: put plugin folders in %APPDATA%\Agora\plugins.</p>
      {/if}
      {#each plugins as p}
        <div class="plugin" class:hidden={p.hidden}>
          <div class="plugin-text">
            <span>{p.name} <span class="muted">{p.version}</span></span>
            <span class="muted small">
              <span class="tag" class:agora={p.source === "Agora"}>{p.source === "Agora" ? "Agora" : "from VSD Craft"}</span>
              {p.hidden ? "hidden" : p.status}
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
  </div>

  <section class="side">
    <div class="triglyph"></div>
    {#if picking}
      <div class="side-head">
        <h2>Pick an action for key <span class="k">{selected + 1}</span></h2>
        {#if info}<button onclick={() => (picking = false)}>Cancel</button>{/if}
      </div>
      <input class="search" placeholder="Search actions" bind:value={search} />
      <p class="muted hint">Click an action, or drag it onto any key. Drag a key onto another key to move it (two used keys swap).</p>
      <div class="actions">
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
                  onclick={() => place(selected, a)}
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
    {:else if info}
      <div class="side-head">
        <div>
          <h2><span class="k">Key {selected + 1}</span> · {info.name}</h2>
          <div class="muted">{info.plugin_name}</div>
        </div>
        <div class="row">
          <button class="primary" onclick={() => (picking = true)}>Change</button>
          <button class="danger" onclick={removeKey}>Remove</button>
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

  <section class="log">
    <h2>Activity</h2>
    {#if log.length === 0}<p class="muted">Plugin messages show up here.</p>{/if}
    <div class="lines">
      {#each log as line}<div class="ostrakon" class:err={line.err}><time>{line.time}</time>{line.text}</div>{/each}
    </div>
  </section>
</main>
