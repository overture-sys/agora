<script lang="ts">
  // The pop-up a Snippets key shows beside the mouse.
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";

  let list = $state<string[]>([]);
  let at = $state(0);

  async function load() {
    list = await invoke<string[]>("snippet_list").catch(() => []);
    at = 0;
  }

  function pick(i: number | null) {
    invoke("pick_snippet", { index: i }).catch(() => {});
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") pick(null);
    else if (e.key === "ArrowDown") at = Math.min(at + 1, list.length - 1);
    else if (e.key === "ArrowUp") at = Math.max(at - 1, 0);
    else if (e.key === "Enter") pick(at);
    else if (/^[1-9]$/.test(e.key) && Number(e.key) <= list.length) pick(Number(e.key) - 1);
    else return;
    e.preventDefault();
  }

  onMount(() => {
    load();
    const un = [listen("snippets-changed", load), getCurrentWindow().onFocusChanged(({ payload }) => { if (!payload) pick(null); })];
    window.addEventListener("keydown", onKeydown);
    return () => {
      un.forEach((p) => p.then((f) => f()));
      window.removeEventListener("keydown", onKeydown);
    };
  });
</script>

<div class="snips" role="listbox" aria-label="Snippets">
  {#each list as s, i}
    <button role="option" aria-selected={i === at} class:at={i === at} onclick={() => pick(i)} onmouseenter={() => (at = i)}>
      <span class="n">{i < 9 ? i + 1 : ""}</span><span class="t">{s}</span>
    </button>
  {/each}
</div>

<style>
  :global(body) { background: var(--panel, #201a15); overflow: hidden; }
  .snips { display: flex; flex-direction: column; padding: 6px; gap: 2px; border: 1px solid var(--line); height: 100vh; box-sizing: border-box; }
  button { display: flex; gap: 10px; align-items: center; text-align: left; min-height: 32px; border: 0; background: transparent; color: var(--text); padding: 4px 8px; border-radius: 6px; }
  button.at { background: var(--accent); color: var(--on-accent, #fff); }
  .n { width: 14px; opacity: .7; font-variant-numeric: tabular-nums; }
  .t { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
</style>
