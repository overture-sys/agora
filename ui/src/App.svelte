<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";

  type Status = { connected: boolean; model: string | null; rows: number; columns: number; message: string };
  type Input = { kind: "key" | "button" | "unknown"; index: number; pressed: boolean; raw_id: number };
  type Action = { plugin: string; plugin_name: string; category: string; action: string; name: string; tooltip: string; icon: string | null };
  type Plugin = { uuid: string; name: string; version: string; source: string; kind: string; status: string };
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
  let log = $state<string[]>([]);
  let brightness = $state(80);
  let ringColor = $state("#0050ff");
  let error = $state("");

  const buttonNames = ["Left", "Middle", "Right"];

  function addLog(line: string) {
    const t = new Date().toLocaleTimeString();
    log = [`${t}  ${line}`, ...log].slice(0, 200);
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

  let dragged: Action | null = null;

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
    <h1>Agora</h1>
    <span class="status" class:on={status.connected}>{status.message || (status.connected ? "Connected" : "Not connected")}</span>
  </header>

  <div class="left">
    <section class="device">
      <div class="grid" style="grid-template-columns: repeat({status.columns}, 1fr)">
        {#each Array(status.rows * status.columns) as _, k}
          <button
            class="key"
            class:pressed={pressedKeys.has(k)}
            class:selected={selected === k}
            onclick={() => loadKey(k)}
            ondragover={(e) => e.preventDefault()}
            ondrop={(e) => {
              e.preventDefault();
              if (dragged) place(k, dragged);
            }}
            title="Key {k + 1}"
          >
            {#if previews[k]}<img src={previews[k]} alt="" draggable="false" />{:else}<span>{k + 1}</span>{/if}
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
        <input type="range" min="0" max="100" bind:value={brightness} onchange={() => call("set_brightness", { percent: brightness })} />
      </label>
      <div class="row">
        <label>Light ring <input type="color" bind:value={ringColor} /></label>
        <button onclick={() => call("set_ring", { color: ringColor })}>Set ring</button>
        <button onclick={() => call("ring_off")}>Ring off</button>
      </div>
      {#if error}<p class="error">{error}</p>{/if}
    </section>

    <section class="plugins">
      <h2>Plugins</h2>
      {#if plugins.length === 0}<p class="muted">No plugins found. Put plugin folders in %APPDATA%\Agora\plugins.</p>{/if}
      {#each plugins as p}
        <div class="plugin">
          <span>{p.name} <span class="muted">{p.version}</span></span>
          <span class="muted">{p.source === "VsdCraft" ? "from VSD Craft" : "Agora"} · {p.status}</span>
        </div>
      {/each}
    </section>
  </div>

  <section class="side">
    {#if picking}
      <div class="side-head">
        <h2>Pick an action for key {selected + 1}</h2>
        {#if info}<button onclick={() => (picking = false)}>Cancel</button>{/if}
      </div>
      <input class="search" placeholder="Search actions" bind:value={search} />
      <p class="muted hint">Click an action, or drag it onto any key.</p>
      <div class="actions">
        {#each groups as [group, list]}
          <h3>{group}</h3>
          {#each list as a}
            <button
              class="action"
              draggable="true"
              ondragstart={() => (dragged = a)}
              ondragend={() => (dragged = null)}
              onclick={() => place(selected, a)}
              title={a.tooltip}
            >
              {#if a.icon}<img src={a.icon} alt="" />{:else}<span class="noicon"></span>{/if}
              <span>{a.name}</span>
            </button>
          {/each}
        {/each}
      </div>
    {:else if info}
      <div class="side-head">
        <div>
          <h2>Key {selected + 1}: {info.name}</h2>
          <div class="muted">{info.plugin_name}</div>
        </div>
        <div class="row">
          <button onclick={() => (picking = true)}>Change</button>
          <button onclick={removeKey}>Remove</button>
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
    {#each log as line}<div>{line}</div>{/each}
  </section>
</main>
