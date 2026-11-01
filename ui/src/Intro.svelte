<script lang="ts">
  // What the window shows when Agora opens: the temple building itself, or
  // the user's own video. Any click or key skips it.
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";

  let { mode, ondone }: { mode: string; ondone: () => void } = $props();
  let video = $state<string | null>(null);
  let leaving = $state(false);
  let el = $state<HTMLVideoElement | null>(null);

  function done() {
    if (leaving) return;
    leaving = true;
    setTimeout(() => {
      if (video) URL.revokeObjectURL(video);
      ondone();
    }, 350);
  }

  onMount(() => {
    const skip = () => done();
    window.addEventListener("keydown", skip);
    let timer: ReturnType<typeof setTimeout> | undefined;
    if (mode === "video") {
      invoke<ArrayBuffer>("intro_video")
        .then((bytes) => {
          video = URL.createObjectURL(new Blob([bytes]));
          timer = setTimeout(done, 120_000);
        })
        .catch(done);
    } else {
      timer = setTimeout(done, 2600);
    }
    return () => {
      clearTimeout(timer);
      window.removeEventListener("keydown", skip);
    };
  });

  $effect(() => {
    if (el && video) {
      // Sound if Windows allows it, otherwise muted.
      el.play().catch(() => {
        if (el) {
          el.muted = true;
          el.play().catch(done);
        }
      });
    }
  });
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="intro" class:leaving onclick={done} title="Click to skip">
  {#if mode === "video"}
    <!-- svelte-ignore a11y_media_has_caption -->
    {#if video}<video bind:this={el} src={video} playsinline onended={done} onerror={done}></video>{/if}
  {:else}
    <svg viewBox="0 0 512 512" width="240" height="240" aria-label="Agora">
      <rect class="tile" x="16" y="16" width="480" height="480" rx="96" />
      <polygon class="roof" points="256,72 440,176 72,176" />
      <rect class="beam" x="80" y="188" width="352" height="22" rx="4" />
      {#each [[96, 232], [208, 232], [320, 232], [96, 344], [208, 344], [320, 344]] as [x, y], i}
        <rect class="key" class:gold={i === 1} x={x} y={y} width="96" height="96" rx="18" style="animation-delay: {0.8 + i * 0.12}s; transform-origin: {x + 48}px {y + 48}px" />
      {/each}
    </svg>
    <div class="word">AGORA</div>
  {/if}
</div>

<style>
  .intro { position: fixed; inset: 0; z-index: 100; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 18px; background: var(--bg); cursor: pointer; transition: opacity .35s; }
  .intro.leaving { opacity: 0; }
  video { max-width: 100%; max-height: 100%; }
  .tile { fill: #F3ECDF; stroke: #DDD0BA; stroke-width: 4; opacity: 0; animation: fade .4s ease-out forwards; }
  .roof { fill: #A94E26; opacity: 0; animation: drop .7s cubic-bezier(.2,.8,.3,1) .1s forwards; }
  .beam { fill: #A94E26; opacity: 0; animation: fade .3s ease-out .55s forwards; }
  .key { fill: #221C18; opacity: 0; transform: scale(.2); animation: pop .35s cubic-bezier(.3,1.5,.5,1) forwards; }
  .key.gold { fill: #D9A441; animation: pop .35s cubic-bezier(.3,1.5,.5,1) forwards, shine .9s ease-in-out 1.7s; }
  .word { font-family: var(--display); letter-spacing: .5em; padding-left: .5em; font-size: 22px; color: var(--accent); opacity: 0; animation: fade .6s ease-out 1.5s forwards; }
  @keyframes fade { to { opacity: 1; } }
  @keyframes drop { from { opacity: 0; transform: translateY(-120px); } to { opacity: 1; transform: none; } }
  @keyframes pop { to { opacity: 1; transform: scale(1); } }
  @keyframes shine { 50% { fill: #FFF3C4; } }
  @media (prefers-reduced-motion: reduce) {
    .tile, .roof, .beam, .key, .word { animation: fade .3s forwards !important; transform: none; }
  }
</style>
