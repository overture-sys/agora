<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";

  type Status = { connected: boolean; model: string | null; rows: number; columns: number; message: string };
  type Input = { kind: "key" | "button" | "unknown"; index: number; pressed: boolean; raw_id: number };

  let status = $state<Status>({ connected: false, model: null, rows: 3, columns: 5, message: "Starting..." });
  let pressedKeys = $state(new Set<number>());
  let pressedButtons = $state(new Set<number>());
  let log = $state<string[]>([]);
  let brightness = $state(80);
  let ringColor = $state("#0050ff");
  let selected = $state<number | null>(null);
  let keyText = $state("");
  let error = $state("");

  const buttonNames = ["Left", "Middle", "Right"];

  function addLog(line: string) {
    log = [line, ...log].slice(0, 40);
  }

  async function call(cmd: string, args: Record<string, unknown> = {}) {
    error = "";
    try {
      await invoke(cmd, args);
    } catch (e) {
      error = String(e);
    }
  }

  function onInput(i: Input) {
    const hex = "0x" + i.raw_id.toString(16).padStart(2, "0");
    const set = i.kind === "button" ? pressedButtons : pressedKeys;
    if (i.kind !== "unknown") {
      const next = new Set(set);
      i.pressed ? next.add(i.index) : next.delete(i.index);
      if (i.kind === "button") pressedButtons = next;
      else pressedKeys = next;
    }
    const what =
      i.kind === "key" ? `key ${i.index + 1}` : i.kind === "button" ? `${buttonNames[i.index]} button` : "unknown control";
    addLog(`${hex}  ${i.pressed ? "down" : "up"}  ${what}`);
  }

  onMount(() => {
    invoke<Status>("dock_status").then((s) => (status = { ...s, message: s.connected ? `${s.model} connected` : status.message }));
    const offs = [
      listen<Status>("dock-status", (e) => (status = e.payload)),
      listen<Input>("dock-input", (e) => onInput(e.payload)),
      listen<string>("dock-error", (e) => (error = e.payload)),
    ];
    return () => offs.forEach((p) => p.then((off) => off()));
  });

  function setKeyText() {
    if (selected === null) return;
    call("set_key_text", { key: selected, text: keyText.replace("|", "\n") });
  }
</script>

<main>
  <header>
    <h1>Agora</h1>
    <span class="status" class:on={status.connected}>{status.message || (status.connected ? "Connected" : "Not connected")}</span>
  </header>

  <section class="device">
    <div class="grid" style="grid-template-columns: repeat({status.columns}, 1fr)">
      {#each Array(status.rows * status.columns) as _, k}
        <button
          class="key"
          class:pressed={pressedKeys.has(k)}
          class:selected={selected === k}
          onclick={() => (selected = k)}
          title="Key {k + 1}">{k + 1}</button
        >
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
    <div class="row">
      <button onclick={() => call("show_test_faces")}>Show test images</button>
      <button onclick={() => call("clear_keys")}>Clear keys</button>
    </div>
    <div class="row">
      <input
        placeholder={selected === null ? "Click a key above first" : `Text for key ${selected + 1} (use | for a new line)`}
        bind:value={keyText}
        disabled={selected === null}
        maxlength="20"
      />
      <button onclick={setKeyText} disabled={selected === null}>Put on key</button>
    </div>
    {#if error}<p class="error">{error}</p>{/if}
  </section>

  <section class="log">
    <h2>Presses</h2>
    {#if log.length === 0}<p class="muted">Press a key or button on the dock.</p>{/if}
    {#each log as line}<div>{line}</div>{/each}
  </section>
</main>
