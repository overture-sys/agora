<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import { onMount } from "svelte";
  import "@fontsource/marcellus-sc/400.css";

  type Status = {
    connected: boolean; model: string | null; model_id: string; model_name: string; rows: number; columns: number;
    buttons: number; dials: number; strip: boolean; ring_leds: number; tested: boolean; message: string;
  };
  type Input = { kind: string; index: number; pressed: boolean; turn: number; raw_id: number };
  type Action = { plugin: string; plugin_name: string; category: string; action: string; name: string; tooltip: string; icon: string | null; group_icon: string | null; controllers: string[] };
  type Plugin = { uuid: string; name: string; version: string; source: string; kind: string; status: string; dir: string; hidden: boolean };
  type Look = { title?: string | null; show_title?: boolean | null; align?: string | null; color?: string | null; font_size?: number | null; icon?: string | null };
  type KeyInfo = { key: number; plugin: string; plugin_name: string; action: string; name: string; settings_url: string | null; builtin: boolean; settings: Record<string, any>; look: Look };
  type NameId = { id: string; name: string };
  type Step = { action: string; settings: Record<string, any> };
  type Save = (patch: Record<string, unknown>) => void;
  type LayoutView = { profiles: NameId[]; profile: string; pages: NameId[]; page: string; top_page: string; path: NameId[]; apps: string[] };
  type Preview = { key: number; image: string | null };
  type Line = { plugin: string; message: string };
  type Settings = { brightness: number; ring_color: string; ring_on: boolean; ring_brightness: number; use_vsd_craft_plugins: boolean; start_with_windows: boolean; ring_colors: string[] };
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
  // Buttons count from 100 and knobs from 200 among a page's keys.
  const BUTTON_BASE = 100;
  const DIAL_BASE = 200;
  const mediaKeys = [
    { id: "play-pause", name: "Play / pause" },
    { id: "next", name: "Next track" },
    { id: "previous", name: "Previous track" },
    { id: "volume-up", name: "Volume up" },
    { id: "volume-down", name: "Volume down" },
    { id: "mute", name: "Mute" },
  ];

  let page = $state<Page>("home");
  let status = $state<Status>({
    connected: false, model: null, model_id: "M18", model_name: "Stream Dock M18", rows: 3, columns: 5,
    buttons: 3, dials: 0, strip: false, ring_leds: 24, tested: true, message: "Looking for your dock...",
  });
  // Keys, buttons and knobs held down on the dock, by their place number.
  let pressedKeys = $state(new Set<number>());
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
  let settings = $state<Settings>({ brightness: 80, ring_color: "#d9794a", ring_on: false, ring_brightness: 100, use_vsd_craft_plugins: false, start_with_windows: false, ring_colors: [] });
  let error = $state("");
  let notice = $state("");
  // The plugin (or "page:id", "profile:id") whose Delete was clicked once and now asks to be sure.
  let confirming = $state<string | null>(null);
  let layout = $state<LayoutView | null>(null);
  // A page or profile name being edited in place.
  let renaming = $state<{ kind: "page" | "profile"; id: string; name: string } | null>(null);
  // The "apps that bring this profile up" panel.
  let appsOpen = $state(false);
  let recentApps = $state<string[]>([]);
  let typedApp = $state("");
  // Importing VSD Craft profiles.
  type VsdProfile = { dir: string; name: string; pages: number; keys: number };
  type ImportReport = { profiles: string[]; keys: number; skipped: string[] };
  let importOpen = $state(false);
  let vsdProfiles = $state<VsdProfile[]>([]);
  let importPick = $state(new Set<string>());
  let importReport = $state<ImportReport | null>(null);

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

  // ---- profiles and pages ----

  async function loadLayout() {
    const before = layout?.page;
    layout = (await invoke<LayoutView | null>("get_layout").catch(() => null)) ?? null;
    // Another page: the open key was on the old one.
    if (before !== undefined && layout && layout.page !== before) loadKey(null);
  }

  async function layoutOp(op: string, id?: string, name?: string, apps?: string[]) {
    renaming = null;
    confirming = null;
    await call("layout_op", { op, id, name, apps });
    await loadLayout();
    loadOverview();
  }

  async function toggleImport() {
    importOpen = !importOpen;
    importReport = null;
    if (importOpen) {
      appsOpen = false;
      vsdProfiles = (await call<VsdProfile[]>("list_vsd_profiles")) ?? [];
      importPick = new Set();
    }
  }

  async function runImport() {
    const report = await call<ImportReport>("import_vsd_profiles", { dirs: [...importPick] });
    if (!report) return;
    importReport = report;
    importPick = new Set();
    await loadLayout();
    refreshPlugins();
  }

  async function toggleApps() {
    appsOpen = !appsOpen;
    if (appsOpen) importOpen = false;
    if (appsOpen) recentApps = (await call<string[]>("recent_apps")) ?? [];
  }

  async function setApps(apps: string[]) {
    if (!layout) return;
    await layoutOp("set-profile-apps", layout.profile, undefined, apps);
  }

  function addApp(app: string) {
    let a = app.trim().toLowerCase();
    if (!a || !layout) return;
    if (!a.includes(".")) a += ".exe";
    typedApp = "";
    if (!layout.apps.includes(a)) setApps([...layout.apps, a]);
  }

  function startRename(kind: "page" | "profile", item: NameId) {
    renaming = { kind, id: item.id, name: item.name };
  }

  function finishRename() {
    if (!renaming) return;
    const r = renaming;
    layoutOp(r.kind === "page" ? "rename-page" : "rename-profile", r.id, r.name);
  }

  function removeWithConfirm(kind: "page" | "profile", id: string) {
    const tag = `${kind}:${id}`;
    if (confirming !== tag) {
      confirming = tag;
      return;
    }
    layoutOp(kind === "page" ? "remove-page" : "remove-profile", id);
  }

  function openFolder(k: number) {
    const ki = keyInfos[k];
    if (ki?.builtin && ki.action === "folder" && ki.settings.page) layoutOp("go-to-page", ki.settings.page);
  }

  const pageName = $derived.by(() => {
    if (!layout) return "";
    const top = layout.pages.find((p) => p.id === layout!.top_page)?.name ?? "";
    return layout.path.length ? layout.path[layout.path.length - 1].name : top;
  });

  // ---- a key's own settings and look ----

  async function saveSettings(patch: Record<string, unknown>) {
    if (!info || selected === null) return;
    info.settings = { ...info.settings, ...patch };
    await call("set_key_settings", { key: selected, settings: info.settings });
    if (info.action === "folder") loadLayout();
    loadOverview();
  }

  async function saveLook(patch: Look) {
    if (!info || selected === null) return;
    const look = { ...info.look, ...patch };
    // Empty means "the action's own".
    for (const k of Object.keys(look) as (keyof Look)[]) if (look[k] === null || look[k] === undefined) delete look[k];
    info.look = look;
    await call("set_key_look", { key: selected, look });
  }

  async function pickProgram(save: Save) {
    const path = await openDialog({ multiple: false, directory: false, title: "Choose an app or file" }).catch(() => null);
    if (typeof path === "string") save({ path });
  }

  async function pickFolderPath(save: Save) {
    const path = await openDialog({ multiple: false, directory: true, title: "Choose a folder" }).catch(() => null);
    if (typeof path === "string") save({ path });
  }

  // ---- multi-action steps ----

  // Settings a new step starts with (the same as the host's defaults).
  const stepDefaults: Record<string, Record<string, unknown>> = {
    open: { path: "", args: "" },
    website: { url: "https://" },
    hotkey: { keys: "" },
    text: { text: "", enter: false },
    media: { key: "play-pause" },
    brightness: { mode: "up", value: 50 },
    ring: { mode: "toggle", color: "#d9794a" },
    "go-to-page": { page: 1 },
    "switch-profile": { profile: "" },
    delay: { ms: 500 },
  };
  const stepChoices = $derived([
    ...actions.filter((a) => a.plugin === "com.agora.builtin" && !["folder", "multi", "toggle", "dial"].includes(a.action)).map((a) => ({ id: a.action, name: a.name })),
    { id: "delay", name: "Pause" },
  ]);

  function stepName(id: string) {
    return id === "delay" ? "Pause" : (actions.find((a) => a.plugin === "com.agora.builtin" && a.action === id)?.name ?? id);
  }

  function stepList(field: string): Step[] {
    return [...((info?.settings[field] ?? []) as Step[])];
  }

  function addStep(field: string, action: string) {
    if (!action) return;
    saveSettings({ [field]: [...stepList(field), { action, settings: { ...(stepDefaults[action] ?? {}) } }] });
  }

  function updateStep(field: string, i: number, patch: Record<string, unknown>) {
    const list = stepList(field);
    list[i] = { ...list[i], settings: { ...list[i].settings, ...patch } };
    saveSettings({ [field]: list });
  }

  function moveStep(field: string, i: number, by: number) {
    const list = stepList(field);
    [list[i], list[i + by]] = [list[i + by], list[i]];
    saveSettings({ [field]: list });
  }

  function removeStep(field: string, i: number) {
    const list = stepList(field);
    list.splice(i, 1);
    saveSettings({ [field]: list });
  }

  /// Reads an image file, shrinks it to the key's size and saves it as the key's icon.
  function pickIcon(e: Event & { currentTarget: HTMLInputElement }) {
    const file = e.currentTarget.files?.[0];
    e.currentTarget.value = "";
    if (!file) return;
    const reader = new FileReader();
    reader.onload = () => {
      const img = new Image();
      img.onload = () => {
        const size = 144;
        const c = document.createElement("canvas");
        c.width = c.height = size;
        const ctx = c.getContext("2d")!;
        const scale = Math.min(size / img.width, size / img.height);
        const w = img.width * scale, h = img.height * scale;
        ctx.drawImage(img, (size - w) / 2, (size - h) / 2, w, h);
        saveLook({ icon: c.toDataURL("image/png") });
      };
      img.onerror = () => (error = "That file isn't an image Agora can read.");
      img.src = String(reader.result);
    };
    reader.readAsDataURL(file);
  }

  /// Turns a key press in the hotkey box into "Ctrl+Shift+M".
  function recordHotkey(e: KeyboardEvent, save: Save) {
    if (e.key === "Tab") return;
    e.preventDefault();
    const named: Record<string, string> = { " ": "Space", ArrowUp: "Up", ArrowDown: "Down", ArrowLeft: "Left", ArrowRight: "Right", Escape: "Esc", Delete: "Del", PageUp: "PgUp", PageDown: "PgDn", PrintScreen: "PrtSc" };
    if (["Control", "Shift", "Alt", "Meta", "OS"].includes(e.key)) return;
    if (e.key === "Backspace" && !e.ctrlKey && !e.altKey && !e.shiftKey && !e.metaKey) {
      save({ keys: "" });
      return;
    }
    const parts = [];
    if (e.ctrlKey) parts.push("Ctrl");
    if (e.shiftKey) parts.push("Shift");
    if (e.altKey) parts.push("Alt");
    if (e.metaKey) parts.push("Win");
    let k = named[e.key] ?? e.key;
    if (/^Key[A-Z]$/.test(e.code)) k = e.code.slice(3);
    else if (/^Digit\d$/.test(e.code)) k = e.code.slice(5);
    else if (k.length === 1) k = k.toUpperCase();
    parts.push(k);
    save({ keys: parts.join("+") });
  }

  async function loadOverview() {
    const slots = allSlots;
    const list = await Promise.all(slots.map((k) => invoke<KeyInfo | null>("key_info", { key: k }).catch(() => null)));
    const next: Record<number, KeyInfo> = {};
    list.forEach((ki, i) => ki && (next[slots[i]] = ki));
    keyInfos = next;
  }

  const isDial = (k: number) => k >= DIAL_BASE;
  /// What the user calls a place: "Key 3", "Left button", "Knob 2".
  function slotName(k: number | null) {
    if (k === null) return "";
    if (k >= DIAL_BASE) return `Knob ${k - DIAL_BASE + 1}`;
    if (k >= BUTTON_BASE) {
      const b = k - BUTTON_BASE;
      return status.buttons === 3 ? `${["Left", "Middle", "Right"][b]} button` : `Button ${b + 1}`;
    }
    return `Key ${k + 1}`;
  }
  /// Whether an action can go on this place: knobs take knob actions only.
  const fitsSlot = (a: Action, k: number) => (a.controllers ?? ["Keypad"]).includes(isDial(k) ? "Encoder" : "Keypad");

  async function place(k: number, a: Action) {
    await call("place_action", { key: k, plugin: a.plugin, action: a.action });
    await loadKey(k);
    loadOverview();
    refreshPlugins();
  }

  /// From the overview: puts an action on the first free key (or knob,
  /// for knob actions).
  async function placeOnFreeKey(a: Action) {
    const free = allSlots.find((k) => fitsSlot(a, k) && !keyInfos[k]);
    if (free === undefined) {
      error = (a.controllers ?? []).includes("Keypad")
        ? "Every key is in use. Drag the action onto a key to replace it."
        : status.dials ? "Every knob is in use. Drag the action onto a knob to replace it." : "This dock has no knobs for that action.";
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
      // Picking for a place: only what fits it. Knob actions only on docks with knobs.
      if (selected !== null ? !fitsSlot(a, selected) : !status.dials && !(a.controllers ?? ["Keypad"]).includes("Keypad")) continue;
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
    await loadLayout();
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
    settings.ring_colors = [];
    settings.ring_on = true;
    await call("set_ring", { color });
  }

  // Painting lights one by one.
  let paint = $state(false);
  let brush = $state("#d9794a");
  let painting = false;

  function lightColor(i: number) {
    return settings.ring_colors.length === 24 ? settings.ring_colors[i] : settings.ring_color;
  }

  async function setLights(colors: string[]) {
    settings.ring_colors = colors;
    settings.ring_on = true;
    await call("set_ring_lights", { colors });
  }

  function paintLight(i: number) {
    const colors = Array.from({ length: 24 }, (_, k) => lightColor(k));
    if (colors[i] === brush) return;
    colors[i] = brush;
    setLights(colors);
  }

  function mix(a: string, b: string, t: number) {
    const p = (h: string, i: number) => parseInt(h.slice(1 + i * 2, 3 + i * 2), 16);
    return "#" + [0, 1, 2].map((i) => Math.round(p(a, i) + (p(b, i) - p(a, i)) * t).toString(16).padStart(2, "0")).join("");
  }

  // Ring presets: 22 ring colours, then the two front lights.
  const ringPresets: { name: string; colors: () => string[] }[] = [
    {
      name: "Rainbow",
      colors: () => {
        const ring = Array.from({ length: 22 }, (_, i) => {
          const h = (i / 22) * 360, f = (n: number) => { const k = (n + h / 60) % 6; return Math.round(255 * (1 - Math.max(0, Math.min(k, 4 - k, 1)))); };
          return "#" + [f(5), f(3), f(1)].map((v) => v.toString(16).padStart(2, "0")).join("");
        });
        return [...ring, ring[0], ring[11]];
      },
    },
    { name: "Meander", colors: () => Array.from({ length: 24 }, (_, i) => (i % 2 ? "#e8bc62" : "#d9794a")) },
    { name: "Dawn", colors: () => { const r = Array.from({ length: 22 }, (_, i) => mix("#d9794a", "#7a2c6e", Math.abs(i - 11) / 11)); return [...r, "#d9794a", "#d9794a"]; } },
    { name: "Aegean", colors: () => { const r = Array.from({ length: 22 }, (_, i) => mix("#2c5a88", "#f3ecdf", Math.abs(i - 11) / 11)); return [...r, "#2c5a88", "#2c5a88"]; } },
  ];

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

  /// A different model brings different keys, buttons and knobs.
  function setStatus(s: Status) {
    const changed = s.model_id !== status.model_id;
    status = s;
    if (changed) {
      loadKey(null);
      loadOverview();
      loadPreviews();
    }
  }

  async function refreshStatus() {
    const s = await invoke<Status>("dock_status").catch(() => null);
    if (s) setStatus({ ...s, message: s.message || status.message });
  }

  function onInput(i: Input) {
    const k = i.kind === "key" ? i.index : i.kind === "button" ? BUTTON_BASE + i.index : i.kind === "dial" || i.kind === "touch" ? DIAL_BASE + i.index : null;
    if (k === null) return;
    const next = new Set(pressedKeys);
    if (i.turn) {
      // A knob step: light it up briefly.
      next.add(k);
      setTimeout(() => {
        const after = new Set(pressedKeys);
        after.delete(k);
        pressedKeys = after;
      }, 150);
    } else if (i.pressed) next.add(k);
    else next.delete(k);
    pressedKeys = next;
  }

  async function loadPreviews() {
    const list = (await invoke<Preview[]>("key_previews").catch(() => [])) ?? [];
    const p: Record<number, string> = {};
    for (const x of list) if (x.image) p[x.key] = x.image;
    previews = p;
  }

  const keyCount = $derived(status.rows * status.columns);
  const buttonSlots = $derived(Array.from({ length: status.buttons }, (_, b) => BUTTON_BASE + b));
  const dialSlots = $derived(Array.from({ length: status.dials }, (_, d) => DIAL_BASE + d));
  const allSlots = $derived([...Array.from({ length: keyCount }, (_, k) => k), ...buttonSlots, ...dialSlots]);
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
    loadLayout();
    const onKeydown = (e: KeyboardEvent) => {
      if (e.key === "Escape" && page === "keys") {
        if (renaming) renaming = null;
        else loadKey(null);
      }
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
      listen<Status>("dock-status", (e) => setStatus(e.payload)),
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
      listen("page-changed", () => {
        loadLayout();
        loadOverview();
      }),
      listen("settings-changed", () => call<Settings>("get_settings").then((s) => s && (settings = s))),
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

{#snippet slot(k: number, editable: boolean, kind: string)}
  <button
    class={kind}
    class:pressed={pressedKeys.has(k)}
    class:selected={editable && selected === k}
    class:drop={dropTarget === k}
    onclick={() => (editable ? clickKey(k) : openKey(k))}
    ondblclick={() => editable && openFolder(k)}
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
    title={editable
      ? keyInfos[k]?.action === "folder" && keyInfos[k]?.builtin
        ? `${slotName(k)}: double-click to open the folder`
        : previews[k] ? `${slotName(k)}: drag to move it` : slotName(k)
      : `Edit ${slotName(k).toLowerCase()}`}
  >
    {#if previews[k]}<img src={previews[k]} alt="" draggable="false" />{:else if kind === "key"}<span class="num">{k + 1}</span>
      <svg class="palm" viewBox="0 0 40 40" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"
        ><path d="M20 34V22" /><path
          d="M20 22c0-8 0-14 0-18M20 22c-2-6-5-11-9-14M20 22c2-6 5-11 9-14M20 22c-5-3-9-6-14-6M20 22c5-3 9-6 14-6"
        /><path d="M12 34c2-3 5-4 8-4s6 1 8 4" /></svg
      >{/if}
  </button>
{/snippet}

{#snippet keyGrid(editable: boolean)}
  <div class="grid" style="grid-template-columns: repeat({status.columns}, 1fr)">
    {#each Array(keyCount) as _, k}
      {@render slot(k, editable, "key")}
    {/each}
  </div>
  {#if buttonSlots.length}
    <div class="buttons">
      {#each buttonSlots as k}{@render slot(k, editable, "round")}{/each}
    </div>
  {/if}
  {#if dialSlots.length}
    <div class="dials" class:strip={status.strip} style="grid-template-columns: repeat({dialSlots.length}, 1fr)">
      {#each dialSlots as k}
        <div class="dial">
          {#if status.strip}<div class="segment">{#if previews[k]}<img src={previews[k]} alt="" />{/if}</div>{/if}
          {@render slot(k, editable, status.strip ? "knob plain" : "knob")}
        </div>
      {/each}
    </div>
  {/if}
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

{#snippet pageBar()}
  {#if layout}
    <div class="pagebar">
      <div class="profile-row">
        <span class="label">Profile</span>
        {#if renaming?.kind === "profile"}
          <!-- svelte-ignore a11y_autofocus -->
          <input class="rename" bind:value={renaming.name} autofocus onkeydown={(e) => e.key === "Enter" && finishRename()} onblur={finishRename} />
        {:else}
          <select value={layout.profile} onchange={(e) => layoutOp("switch-profile", e.currentTarget.value)} aria-label="Profile">
            {#each layout.profiles as p}<option value={p.id}>{p.name}</option>{/each}
          </select>
          <button class="small" onclick={() => startRename("profile", layout!.profiles.find((p) => p.id === layout!.profile)!)} title="Rename this profile">Rename</button>
          <button class="small" onclick={() => layoutOp("add-profile", undefined, `Profile ${layout!.profiles.length + 1}`)}>New</button>
          <button class="small" class:on={appsOpen} onclick={toggleApps} aria-expanded={appsOpen}
            title="Apps that bring this profile up">Apps{layout.apps.length ? ` (${layout.apps.length})` : ""}</button>
          <button class="small" class:on={importOpen} onclick={toggleImport} aria-expanded={importOpen}
            title="Bring in profiles from VSD Craft">Import</button>
          {#if layout.profiles.length > 1}
            <button class="small danger" class:sure={confirming === `profile:${layout.profile}`} onclick={() => removeWithConfirm("profile", layout!.profile)}
              >{confirming === `profile:${layout.profile}` ? "Sure? Delete" : "Delete"}</button
            >
          {/if}
        {/if}
      </div>
      {#if importOpen}
        <div class="apps-panel">
          {#if importReport}
            <p><b>Imported {importReport.profiles.join(", ")}</b> with {importReport.keys} keys. Pick them from the profile list.</p>
            {#if importReport.skipped.length}
              <p class="muted small">Left out, because Agora can't run them yet: {importReport.skipped.join(", ")}.</p>
            {/if}
            <button class="small" onclick={() => (importOpen = false)}>Done</button>
          {:else}
            <p class="muted small">Your M18 profiles from VSD Craft. Each one you pick becomes a new Agora profile; VSD Craft's files aren't changed.</p>
            {#if vsdProfiles.length === 0}<p class="muted small">No VSD Craft profiles for the M18 found.</p>{/if}
            <div class="import-list">
              {#each vsdProfiles as vp}
                <label class="check">
                  <input type="checkbox" checked={importPick.has(vp.dir)}
                    onchange={(e) => { const n = new Set(importPick); e.currentTarget.checked ? n.add(vp.dir) : n.delete(vp.dir); importPick = n; }} />
                  <span>{vp.name || "(no name)"} <span class="muted small">{vp.pages} {vp.pages === 1 ? "page" : "pages"}</span></span>
                </label>
              {/each}
            </div>
            <div class="row">
              <button class="small primary" disabled={importPick.size === 0} onclick={runImport}>Import {importPick.size || ""}</button>
              <button class="small" onclick={() => (importOpen = false)}>Cancel</button>
            </div>
          {/if}
        </div>
      {/if}
      {#if appsOpen}
        <div class="apps-panel">
          <p class="muted small">This profile comes up by itself while one of these apps is in front, and the one you chose comes back after.</p>
          <div class="chips">
            {#each layout.apps as a}
              <span class="chip">{a}<button onclick={() => setApps(layout!.apps.filter((x) => x !== a))} aria-label="Remove {a}">×</button></span>
            {:else}
              <span class="muted small">No apps yet.</span>
            {/each}
          </div>
          <div class="row">
            <select value="" onchange={(e) => { addApp(e.currentTarget.value); e.currentTarget.value = ""; }} aria-label="Recent apps">
              <option value="" disabled>Apps you used lately…</option>
              {#each recentApps.filter((a) => !layout!.apps.includes(a)) as a}<option value={a}>{a}</option>{/each}
            </select>
            <input class="rename" placeholder="or type one, e.g. obs64.exe" bind:value={typedApp} onkeydown={(e) => e.key === "Enter" && addApp(typedApp)} />
            <button class="small" onclick={() => addApp(typedApp)} disabled={!typedApp.trim()}>Add</button>
          </div>
        </div>
      {/if}
      <div class="tabs" role="tablist" aria-label="Pages">
        {#each layout.pages as p}
          {@const active = p.id === layout.top_page}
          {#if renaming?.kind === "page" && renaming.id === p.id}
            <!-- svelte-ignore a11y_autofocus -->
            <input class="rename tab-input" bind:value={renaming.name} autofocus onkeydown={(e) => e.key === "Enter" && finishRename()} onblur={finishRename} />
          {:else}
            <button
              class="tab"
              class:active
              role="tab"
              aria-selected={active}
              onclick={() => (active && layout!.path.length === 0 ? startRename("page", p) : layoutOp("go-to-page", p.id))}
              title={active ? "Click to rename" : `Show ${p.name}`}>{p.name}</button
            >
          {/if}
        {/each}
        <button class="tab add" onclick={() => layoutOp("add-page")} title="Add a page">+ Page</button>
        {#if layout.pages.length > 1 && layout.path.length === 0}
          <button class="tab remove" class:sure={confirming === `page:${layout.top_page}`} onclick={() => removeWithConfirm("page", layout!.top_page)}
            title="Delete this page and the folders on it">{confirming === `page:${layout.top_page}` ? "Sure? Delete page" : "Delete page"}</button
          >
        {/if}
      </div>
      {#if layout.path.length}
        <nav class="crumbs" aria-label="Folder">
          <button class="small" onclick={() => layoutOp("go-to-page", layout!.path.length > 1 ? layout!.path[layout!.path.length - 2].id : layout!.top_page)}>← Back</button>
          <button class="crumb" onclick={() => layoutOp("go-to-page", layout!.top_page)}>{layout.pages.find((p) => p.id === layout!.top_page)?.name}</button>
          {#each layout.path as f, i}
            <span class="sep">›</span>
            {#if i === layout.path.length - 1}<b>{f.name}</b>{:else}<button class="crumb" onclick={() => layoutOp("go-to-page", f.id)}>{f.name}</button>{/if}
          {/each}
        </nav>
      {/if}
    </div>
  {/if}
{/snippet}

{#snippet actionForm(action: string, st: Record<string, any>, save: Save)}
  <div class="form">
    {#if action === "folder"}
      <label>Folder name <input value={st.name ?? ""} onchange={(e) => save({ name: e.currentTarget.value })} /></label>
      <button class="primary" onclick={() => layoutOp("go-to-page", st.page)}>Open folder</button>
      <p class="muted small">Its keys get their own page, with a Back key on key 1. Double-click the folder key to open it too.</p>
    {:else if action === "go-to-page"}
      <label>Page
        <select value={st.page ?? 1} onchange={(e) => save({ page: Number(e.currentTarget.value) })}>
          {#each layout?.pages ?? [] as p, i}<option value={i + 1}>{i + 1}. {p.name}</option>{/each}
        </select>
      </label>
    {:else if action === "switch-profile"}
      <label>Profile
        <select value={st.profile ?? ""} onchange={(e) => save({ profile: e.currentTarget.value })}>
          <option value="" disabled>Choose a profile</option>
          {#each layout?.profiles ?? [] as p}<option value={p.id}>{p.name}</option>{/each}
        </select>
      </label>
    {:else if action === "open"}
      <label>App, file or folder
        <span class="row grow"><input value={st.path ?? ""} placeholder="C:\Program Files\..." onchange={(e) => save({ path: e.currentTarget.value })} />
          <button onclick={() => pickProgram(save)}>File…</button><button onclick={() => pickFolderPath(save)}>Folder…</button></span>
      </label>
      <label>Arguments <span class="muted small">(programs only)</span><input value={st.args ?? ""} onchange={(e) => save({ args: e.currentTarget.value })} /></label>
    {:else if action === "website"}
      <label>Web address <input value={st.url ?? ""} placeholder="https://" onchange={(e) => save({ url: e.currentTarget.value })} /></label>
    {:else if action === "hotkey"}
      <label>Keys
        <input class="hotkey" value={st.keys ?? ""} placeholder="Click here, then press the keys" readonly onkeydown={(e) => recordHotkey(e, save)} />
      </label>
      <p class="muted small">Press the combination, such as Ctrl+Shift+M. Backspace clears it.</p>
    {:else if action === "text"}
      <label>Text <textarea rows="4" value={st.text ?? ""} onchange={(e) => save({ text: e.currentTarget.value })}></textarea></label>
      <label class="check"><input type="checkbox" checked={!!st.enter} onchange={(e) => save({ enter: e.currentTarget.checked })} /> Press Enter afterwards</label>
    {:else if action === "media"}
      <label>Key
        <select value={st.key ?? "play-pause"} onchange={(e) => save({ key: e.currentTarget.value })}>
          {#each mediaKeys as m}<option value={m.id}>{m.name}</option>{/each}
        </select>
      </label>
    {:else if action === "brightness"}
      <label>Each press
        <select value={st.mode ?? "up"} onchange={(e) => save({ mode: e.currentTarget.value })}>
          <option value="up">Brighter (+10%)</option><option value="down">Dimmer (-10%)</option><option value="set">Set to a level</option>
        </select>
      </label>
      {#if st.mode === "set"}
        <label class="slider"><span>Level <b>{st.value ?? 50}%</b></span>
          <input type="range" min="0" max="100" value={st.value ?? 50} style="--fill: {st.value ?? 50}%" onchange={(e) => save({ value: Number(e.currentTarget.value) })} />
        </label>
      {/if}
    {:else if action === "ring"}
      <label>Each press
        <select value={st.mode ?? "toggle"} onchange={(e) => save({ mode: e.currentTarget.value })}>
          <option value="toggle">Turn on or off</option><option value="on">Turn on</option><option value="off">Turn off</option><option value="color">Set a colour</option>
        </select>
      </label>
      {#if st.mode === "color"}
        <label class="check">Colour <input type="color" value={st.color ?? "#d9794a"} onchange={(e) => save({ color: e.currentTarget.value })} /></label>
      {/if}
    {:else if action === "delay"}
      <label>Pause for (milliseconds) <input type="number" min="0" max="60000" step="100" value={st.ms ?? 500} onchange={(e) => save({ ms: Number(e.currentTarget.value) })} /></label>
    {:else if action === "multi"}
      {@render stepsEditor("steps", "Steps, run in order on each press")}
    {:else if action === "toggle"}
      {@render stepsEditor("steps", "First press")}
      {@render stepsEditor("steps_b", "Second press")}
    {:else if action === "dial"}
      <p class="muted small">Each step of a turn runs the list once. Volume up and down from Media work well here.</p>
      {@render stepsEditor("left", "Turn left")}
      {@render stepsEditor("right", "Turn right")}
      {@render stepsEditor("press", status.strip ? "Press or tap the screen above" : "Press")}
    {:else}
      <p class="muted">This action has no settings.</p>
    {/if}
  </div>
{/snippet}

{#snippet stepsEditor(field: string, label: string)}
  {@const list = (info?.settings[field] ?? []) as Step[]}
  <div class="steps">
    <h3>{label}</h3>
    {#if list.length === 0}<p class="muted small">No steps yet.</p>{/if}
    {#each list as step, i}
      <div class="step">
        <div class="step-head">
          <b>{i + 1}. {stepName(step.action)}</b>
          <span class="row">
            <button class="small" disabled={i === 0} onclick={() => moveStep(field, i, -1)} aria-label="Move up">↑</button>
            <button class="small" disabled={i === list.length - 1} onclick={() => moveStep(field, i, 1)} aria-label="Move down">↓</button>
            <button class="small danger" onclick={() => removeStep(field, i)} aria-label="Remove step">×</button>
          </span>
        </div>
        {@render actionForm(step.action, step.settings ?? {}, (patch) => updateStep(field, i, patch))}
      </div>
    {/each}
    <select class="add-step" value="" onchange={(e) => { addStep(field, e.currentTarget.value); e.currentTarget.value = ""; }} aria-label="Add a step">
      <option value="" disabled>+ Add a step</option>
      {#each stepChoices as c}<option value={c.id}>{c.name}</option>{/each}
    </select>
  </div>
{/snippet}

{#snippet lookEditor(ki: KeyInfo)}
  {@const lk = ki.look}
  <details class="look" open={ki.builtin || !ki.settings_url}>
    <summary>Title and icon</summary>
    <div class="form">
      <label>Title
        <input value={lk.title ?? ""} placeholder="The action's own title" onchange={(e) => saveLook({ title: e.currentTarget.value === "" ? null : e.currentTarget.value })} />
      </label>
      <div class="row wrap">
        <label class="check"><input type="checkbox" checked={lk.show_title ?? true} onchange={(e) => saveLook({ show_title: e.currentTarget.checked })} /> Show title</label>
        <select value={lk.align ?? ""} onchange={(e) => saveLook({ align: e.currentTarget.value || null })} aria-label="Title position">
          <option value="">Position: the action's</option><option value="top">Top</option><option value="middle">Middle</option><option value="bottom">Bottom</option>
        </select>
        <label class="check">Colour <input type="color" value={lk.color ?? "#ffffff"} onchange={(e) => saveLook({ color: e.currentTarget.value })} /></label>
      </div>
      <div class="row wrap">
        <label class="button-like">Choose icon…<input type="file" accept="image/*" onchange={pickIcon} hidden /></label>
        {#if lk.icon}<button onclick={() => saveLook({ icon: null })}>Use the action's icon</button>{/if}
        {#if Object.keys(lk).length}<button onclick={() => { info && (info.look = {}); call("set_key_look", { key: selected, look: {} }); }}>Reset all</button>{/if}
      </div>
    </div>
  </details>
{/snippet}

{#snippet activity(limit: number)}
  {#if log.length === 0}<p class="muted">Plugin messages show up here.</p>{/if}
  <div class="lines">
    {#each log.slice(0, limit) as line}<div class="ostrakon" class:err={line.err}><time>{line.time}</time>{line.text}</div>{/each}
  </div>
{/snippet}

<svelte:window onpointerup={() => (painting = false)} />

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
          <span class="card-label">Screen brightness{status.ring_leds ? ` · ${status.ring_leds >= 24 ? "ring" : "lights"} ${settings.ring_on ? "on" : "off"}` : ""}</span>
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
          {@render pageBar()}
          {@render keyGrid(true)}
          {#if error}<p class="error">{error}</p>{/if}
        </section>

        <section class="side">
          <div class="triglyph"></div>
          {#if selected === null}
            <div class="side-head">
              <div>
                <h2>{pageName || "Your keys"}</h2>
                <div class="muted">{Object.keys(keyInfos).length} of {allSlots.length} {allSlots.length > keyCount ? "keys, buttons and knobs" : "keys"} set up. Click one to edit it.</div>
              </div>
            </div>
            {#if Object.keys(keyInfos).length > 0}
              <div class="key-list">
                {#each Object.values(keyInfos) as ki (ki.key)}
                  <button class="key-row" onclick={() => loadKey(ki.key)}>
                    {#if previews[ki.key]}<img src={previews[ki.key]} alt="" />{:else}<span class="noicon"></span>{/if}
                    <span class="key-row-text"><b><span class="k">{slotName(ki.key)}</span> · {ki.name}</b><span class="muted small">{ki.plugin_name}</span></span>
                  </button>
                {/each}
              </div>
            {/if}
            <h2 class="lib-head">Action library</h2>
            <p class="muted hint">Drag an action onto a key{status.buttons ? " or button" : ""}{status.dials ? " or knob" : ""}, or click it to use the first free one. Drag keys onto each other to move or swap them.</p>
            {@render library()}
          {:else if picking}
            <div class="side-head">
              <h2>Pick an action for <span class="k">{slotName(selected).toLowerCase()}</span></h2>
              <div class="row">
                {#if info}<button onclick={() => (picking = false)}>Cancel</button>{/if}
                <button onclick={() => loadKey(null)}>All keys</button>
              </div>
            </div>
            <p class="muted hint">{isDial(selected) ? "Knobs take Knob, or plugin actions made for knobs." : `Click an action to put it on ${slotName(selected).toLowerCase()}, or drag it onto any key.`}</p>
            {@render library()}
          {:else if info}
            <div class="side-head">
              <div>
                <h2><span class="k">{slotName(selected)}</span> · {info.name}</h2>
                <div class="muted">{info.plugin_name}</div>
              </div>
              <div class="row">
                <button class="primary" onclick={() => (picking = true)}>Change</button>
                <button class="danger" onclick={removeKey}>Remove</button>
                <button onclick={() => loadKey(null)} title="Back to the overview (Esc)">All keys</button>
              </div>
            </div>
            {#if info.builtin}
              {@render actionForm(info.action, info.settings, saveSettings)}
            {/if}
            {#if selected !== null && (selected < BUTTON_BASE || (isDial(selected) && status.strip))}
              {@render lookEditor(info)}
            {/if}
            {#if info.settings_url}
              {#key info.settings_url}
                <iframe class="pi" src={info.settings_url} title="Settings for {info.name}"></iframe>
              {/key}
            {:else if !info.builtin}
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
          <p class="muted small">{status.model_name}{status.connected ? "" : " (last connected)"}{status.tested ? "" : ". Agora hasn't been tried on this model yet; tell us what works and what doesn't."}</p>
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

        {#if status.ring_leds > 0}
        <section class="ring-card">
          <div class="side-head">
            <h2>{status.ring_leds >= 24 ? "Light ring" : "Side lights"}</h2>
            <label class="switch-row tight">
              <input type="checkbox" role="switch" checked={settings.ring_on} onchange={toggleRing} />
              <span>{settings.ring_on ? "On" : "Off"}</span>
            </label>
          </div>
          {#if status.ring_leds >= 24}
          <svg class="ring-preview" viewBox="0 0 220 152" role="img" aria-label="Light ring preview">
            <rect x="34" y="26" width="152" height="88" rx="14" fill="var(--key)" />
            {#each ringLeds as l, i}
              <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
              <circle cx={l.x} cy={l.y} r={paint ? 7 : 5} fill={settings.ring_on ? lightColor(i) : "var(--line)"}
                class:paintable={paint}
                opacity={settings.ring_on ? 0.35 + (0.65 * settings.ring_brightness) / 100 : 1}
                onpointerdown={() => { if (paint) { painting = true; paintLight(i); } }}
                onpointerenter={() => painting && paintLight(i)} />
            {/each}
            {#each [96, 124] as x, f}
              <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
              <rect x={x - 7} y="140" width="14" height={paint ? 8 : 5} rx="2" fill={settings.ring_on ? lightColor(22 + f) : "var(--line)"}
                class:paintable={paint}
                onpointerdown={() => { if (paint) { painting = true; paintLight(22 + f); } }}
                onpointerenter={() => painting && paintLight(22 + f)} />
            {/each}
          </svg>
          <div class="seg" role="group" aria-label="How colours apply">
            <button class:active={!paint} onclick={() => (paint = false)}>Whole ring</button>
            <button class:active={paint} onclick={() => { paint = true; brush = settings.ring_color; }}>Paint lights</button>
          </div>
          {#if paint}<p class="muted small">Pick a colour, then click or drag over the lights.</p>{/if}
          {/if}
          <div class="swatches" role="group" aria-label="Ring colour">
            {#each ringSwatches as sw}
              <button
                class="swatch-btn"
                class:chosen={paint ? brush === sw.color : settings.ring_on && !settings.ring_colors.length && settings.ring_color.toLowerCase() === sw.color}
                style="--sw: {sw.color}"
                title={sw.name}
                aria-label={sw.name}
                onclick={() => (paint ? (brush = sw.color) : setRing(sw.color))}
              ></button>
            {/each}
            <label class="swatch-btn custom" title="Pick any colour">
              <input type="color" value={paint ? brush : settings.ring_color} onchange={(e) => (paint ? (brush = e.currentTarget.value) : setRing(e.currentTarget.value))} aria-label="Pick any colour" />
            </label>
          </div>
          {#if status.ring_leds >= 24}
          <div class="row wrap presets">
            <span class="muted small">Presets</span>
            {#each ringPresets as pr}<button class="small" onclick={() => setLights(pr.colors())}>{pr.name}</button>{/each}
          </div>
          {/if}
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
        {/if}

        <section class="screen-card">
          <h2>Agora</h2>
          <label class="switch-row">
            <input type="checkbox" role="switch" checked={settings.start_with_windows}
              onchange={async (e) => {
                const on = e.currentTarget.checked;
                await call("set_start_with_windows", { on });
                settings = (await call<Settings>("get_settings")) ?? settings;
              }} />
            <span>
              Start with Windows
              <span class="muted small">Agora starts in the tray when you sign in, so the dock works straight away.</span>
            </span>
          </label>
        </section>
      </div>
    {/if}
  </main>
</div>
