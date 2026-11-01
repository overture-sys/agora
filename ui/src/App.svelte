<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import { onMount, tick } from "svelte";
  import Intro from "./Intro.svelte";
  import "@fontsource/marcellus-sc/400.css";
  import "@fontsource/inter/400.css";
  import "@fontsource/inter/500.css";
  import "@fontsource/inter/600.css";
  import "@fontsource/gfs-didot/greek-400.css";

  type Status = {
    connected: boolean; model: string | null; model_id: string; model_name: string; rows: number; columns: number;
    buttons: number; dials: number; strip: boolean; ring_leds: number; tested: boolean; message: string;
  };
  type Input = { kind: string; index: number; pressed: boolean; turn: number; raw_id: number };
  type Action = { plugin: string; plugin_name: string; category: string; action: string; name: string; tooltip: string; icon: string | null; group_icon: string | null; controllers: string[] };
  type Plugin = { uuid: string; name: string; version: string; source: string; kind: string; status: string; dir: string; hidden: boolean };
  type Look = { title?: string | null; show_title?: boolean | null; align?: string | null; color?: string | null; font_size?: number | null; icon?: string | null };
  type KeyInfo = { key: number; plugin: string; plugin_name: string; action: string; name: string; settings_url: string | null; builtin: boolean; settings: Record<string, any>; look: Look; hold?: Step[] };
  type NameId = { id: string; name: string };
  type Step = { action: string; settings: Record<string, any> };
  type Save = (patch: Record<string, unknown>) => void;
  type LayoutView = { profiles: NameId[]; profile: string; pages: NameId[]; page: string; top_page: string; path: NameId[]; apps: string[] };
  type Preview = { key: number; image: string | null };
  type PageOverview = { layout: LayoutView; keys: KeyInfo[] };
  type Line = { plugin: string; message: string };
  type Settings = { brightness: number; ring_color: string; ring_on: boolean; ring_brightness: number; use_vsd_craft_plugins: boolean; start_with_windows: boolean; ring_colors: string[]; theme?: string; saver?: string; saver_minutes?: number; saver_gif?: string; dock_intro?: boolean; intro?: string; intro_video?: string; intro_seen_version?: string; hold_ms?: number; app_version?: string };
  type LogLine = { time: string; text: string; err: boolean };
  type Page = "home" | "keys" | "plugins" | "dock" | "settings";

  const pages: { id: Page; label: string }[] = [
    { id: "home", label: "Home" },
    { id: "keys", label: "Keys" },
    { id: "plugins", label: "Plugins" },
    { id: "dock", label: "Dock" },
    { id: "settings", label: "Settings" },
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
  // What was just removed, with a way to put it back for a few seconds.
  type Toast = { text: string; undo: () => Promise<void>; commit?: () => Promise<void> };
  let toast = $state<Toast | null>(null);
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  let noticeTimer: ReturnType<typeof setTimeout> | undefined;
  let layout = $state<LayoutView | null>(null);
  // A page or profile name being edited in place.
  let renaming = $state<{ kind: "page" | "profile"; id: string; name: string } | null>(null);
  // The "apps that bring this profile up" panel.
  let appsOpen = $state(false);
  let profileMenu = $state(false);
  let recentApps = $state<string[]>([]);
  let typedApp = $state("");
  // Importing VSD Craft profiles.
  type VsdProfile = { dir: string; name: string; pages: number; keys: number };
  type ImportReport = { profiles: string[]; keys: number; skipped: string[] };
  let importOpen = $state(false);
  let vsdProfiles = $state<VsdProfile[]>([]);
  let importPick = $state(new Set<string>());
  let importReport = $state<ImportReport | null>(null);
  // VSD Craft profiles found on this PC, offered on Home.
  let vsdFound = $state(0);
  let vsdOfferHidden = $state(false);
  try {
    vsdOfferHidden = localStorage.getItem("agora.vsdOfferHidden") === "1";
  } catch {}

  function hideVsdOffer() {
    vsdOfferHidden = true;
    try {
      localStorage.setItem("agora.vsdOfferHidden", "1");
    } catch {}
  }

  async function startImport() {
    page = "keys";
    if (!importOpen) await toggleImport();
  }

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

  // Changes that get a quiet "Saved" in the status area.
  const savingCommands = new Set([
    "place_action", "move_key", "set_key_settings", "set_key_look", "layout_op", "set_brightness", "set_ring",
    "set_ring_lights", "ring_off", "set_ring_brightness", "set_start_with_windows", "set_use_vsd_craft_plugins",
    "import_vsd_profiles", "show_plugin", "set_key_hold", "set_screen_saver", "set_intro",
  ]);

  function flash(text: string) {
    notice = text;
    clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => (notice = ""), 2500);
  }

  async function call<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T | undefined> {
    error = "";
    try {
      const r = await invoke<T>(cmd, args);
      if (savingCommands.has(cmd) && !(cmd === "layout_op" && args.op === "go-to-page")) flash("Saved");
      return r;
    } catch (e) {
      const msg = String(e);
      // Saved for later rather than failed.
      if (msg.startsWith("Saved.")) flash(msg);
      else error = msg;
    }
  }

  /// Shows what was removed with an Undo button. `commit` runs when the
  /// toast goes away without Undo (deleting a plugin's folder for real).
  function showUndo(t: Toast) {
    endToast();
    toast = t;
    toastTimer = setTimeout(endToast, 6000);
  }

  function endToast() {
    clearTimeout(toastTimer);
    const t = toast;
    toast = null;
    t?.commit?.();
  }

  async function undoToast() {
    clearTimeout(toastTimer);
    const t = toast;
    toast = null;
    if (t) await t.undo();
  }

  /// Puts back the last removed key, page, profile or plugin.
  async function hostUndo() {
    const ok = await call<boolean>("undo");
    if (!ok) error = "That can't be undone any more, because something else changed since.";
    await refreshAll();
    loadPreviews();
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

  // One question to the host per refresh: the layout and every place's
  // action together. A refresh asked for while one runs waits for it and
  // then runs once.
  let refreshing: Promise<void> | null = null;
  let refreshAgain = false;
  // Page changes this window asked for: the host's "page changed" for them
  // needs no second refresh.
  let ownChangeUntil = 0;

  function refreshPage(): Promise<void> {
    if (refreshing) {
      refreshAgain = true;
      return refreshing;
    }
    refreshing = (async () => {
      do {
        refreshAgain = false;
        const o = await invoke<PageOverview | null>("page_overview").catch(() => null);
        if (!o) continue;
        const before = layout?.page;
        layout = o.layout;
        const next: Record<number, KeyInfo> = {};
        for (const ki of o.keys) next[ki.key] = ki;
        keyInfos = next;
        // Another page: the open key was on the old one.
        if (before !== undefined && o.layout.page !== before) loadKey(null);
      } while (refreshAgain);
      refreshing = null;
    })();
    return refreshing;
  }

  async function layoutOp(op: string, id?: string, name?: string, apps?: string[]) {
    renaming = null;
    // Show the new tab straight away; the host catches up.
    if (op === "go-to-page" && id && layout?.pages.some((p) => p.id === id)) {
      layout = { ...layout, top_page: id, page: id, path: [] };
      loadKey(null);
    }
    ownChangeUntil = performance.now() + 1000;
    await call("layout_op", { op, id, name, apps });
    await refreshPage();
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
    await refreshPage();
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

  async function removeWithUndo(kind: "page" | "profile", id: string) {
    const list = kind === "page" ? layout?.pages : layout?.profiles;
    const name = list?.find((p) => p.id === id)?.name ?? "";
    await layoutOp(kind === "page" ? "remove-page" : "remove-profile", id);
    if (!error) showUndo({ text: `Deleted the ${kind} ${name}.`, undo: hostUndo });
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
    refreshPage();
  }

  async function saveLook(patch: Look) {
    if (!info || selected === null) return;
    const look = { ...info.look, ...patch };
    // Empty means "the action's own".
    for (const k of Object.keys(look) as (keyof Look)[]) if (look[k] === null || look[k] === undefined) delete look[k];
    info.look = look;
    await call("set_key_look", { key: selected, look });
  }

  async function pickProgram(save: Save, autoIcon = false) {
    const path = await openDialog({ multiple: false, directory: false, title: "Choose an app or file" }).catch(() => null);
    if (typeof path === "string") {
      save({ path });
      if (autoIcon) useAppIcon(path);
    }
  }

  /// An Open key without its own icon takes the program's.
  async function useAppIcon(path: string) {
    if (!info || info.action !== "open" || info.look.icon || !/\.(exe|lnk)$/i.test(path.trim().replace(/"/g, ""))) return;
    const icon = await invoke<string | null>("app_icon", { path }).catch(() => null);
    if (icon && info && !info.look.icon) saveLook({ icon });
  }

  async function pickFile(title: string, name: string, extensions: string[]) {
    const path = await openDialog({ multiple: false, directory: false, title, filters: [{ name, extensions }] }).catch(() => null);
    return typeof path === "string" ? path : null;
  }

  // Sound outputs and apps playing sound, for the Audio keys.
  function audioOutputs() {
    return invoke<{ id: string; name: string; default: boolean }[]>("audio_outputs").catch(() => []);
  }
  let audioApps = $state<string[]>([]);
  async function loadAudioApps() {
    audioApps = await invoke<string[]>("audio_apps").catch(() => []);
  }

  async function pickSound(save: Save) {
    const path = await openDialog({ multiple: false, directory: false, title: "Choose a sound", filters: [{ name: "Sounds", extensions: ["mp3", "wav", "wma", "m4a", "aac"] }] }).catch(() => null);
    if (typeof path === "string") save({ path });
  }

  // The password box is never saved in the layout; it goes straight to Windows.
  let pwDraft = $state("");
  let pwTick = $state(0);
  function pwSaved(name: string, _tick: number) {
    return invoke<boolean>("password_saved", { name }).catch(() => false);
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
    "quick-control": { what: "show-desktop" },
    "quick-tool": { tool: "calculator" },
    "close-window": {},
    mouse: { mode: "click", button: "left", double: false, amount: 3, x: 0, y: 0, relative: true },
    browser: { what: "back" },
    password: { name: "", enter: false },
    sound: { mode: "play", path: "", volume: 100 },
    udp: { host: "", port: 0, message: "", hex: false },
    "dock-sleep": {},
    "audio-output": { mode: "cycle", device: "", device_name: "" },
    "app-volume": { app: "", what: "up", step: 10 },
    "web-request": { method: "POST", url: "https://", body: "", content_type: "application/json" },
    "run-command": { command: "", show_output: false },
    snippets: { snippets: [], enter: false },
    window: { what: "next-screen" },
    delay: { ms: 500 },
  };
  const stepChoices = $derived([
    ...actions.filter((a) => a.plugin === "com.agora.builtin" && !["folder", "multi", "toggle", "dial", "page-indicator", "cpu", "memory", "disk", "gpu", "gpu-memory", "ip", "random", "hotkey-switch", "clock", "timer", "stopwatch", "counter"].includes(a.action)).map((a) => ({ id: a.action, name: a.name })),
    { id: "delay", name: "Pause" },
  ]);

  function stepName(id: string) {
    return id === "delay" ? "Pause" : (actions.find((a) => a.plugin === "com.agora.builtin" && a.action === id)?.name ?? id);
  }

  // "hold" is the key's own hold list, kept beside its settings.
  function stepList(field: string): Step[] {
    if (field === HOLD) return [...(info?.hold ?? [])];
    return [...((info?.settings[field] ?? []) as Step[])];
  }

  const HOLD = "\u0000hold";

  function saveSteps(field: string, list: Step[]) {
    if (field === HOLD) {
      if (!info || selected === null) return;
      info.hold = list;
      call("set_key_hold", { key: selected, steps: list });
    } else {
      saveSettings({ [field]: list });
    }
  }

  function addStep(field: string, action: string) {
    if (!action) return;
    saveSteps(field, [...stepList(field), { action, settings: { ...(stepDefaults[action] ?? {}) } }]);
  }

  function updateStep(field: string, i: number, patch: Record<string, unknown>) {
    const list = stepList(field);
    list[i] = { ...list[i], settings: { ...list[i].settings, ...patch } };
    saveSteps(field, list);
  }

  function moveStep(field: string, i: number, by: number) {
    const list = stepList(field);
    [list[i], list[i + by]] = [list[i + by], list[i]];
    saveSteps(field, list);
  }

  function removeStep(field: string, i: number) {
    const list = stepList(field);
    list.splice(i, 1);
    saveSteps(field, list);
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
    refreshPage();
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
    await removeKeyAt(selected);
  }

  let introMode = $state<string | null>(null);
  const savers = [
    { id: "off", name: "Off" },
    { id: "matrix", name: "Matrix rain" },
    { id: "temple", name: "Agora temple" },
    { id: "stars", name: "Starfield" },
    { id: "clock", name: "Big clock" },
    { id: "gif", name: "Your own GIF" },
  ];
  async function saveSaver(patch: Partial<Settings>) {
    settings = { ...settings, ...patch };
    await call("set_screen_saver", { mode: settings.saver ?? "off", minutes: settings.saver_minutes ?? 10, gif: settings.saver_gif ?? "" });
  }
  async function saveIntro(patch: Partial<Settings>) {
    settings = { ...settings, ...patch };
    await call("set_intro", { intro: settings.intro ?? "update", video: settings.intro_video ?? "", dockIntro: settings.dock_intro ?? true });
  }

  // Right-click menu on a key (or Shift+F10 / the menu key on a focused key).
  let keyMenu = $state<{ x: number; y: number; k: number; pages: boolean } | null>(null);
  let menuOpener: HTMLElement | null = null;
  let clipboardFull = $state(false);

  async function openKeyMenu(e: MouseEvent | KeyboardEvent, k: number) {
    e.preventDefault();
    menuOpener = e.currentTarget instanceof HTMLElement ? e.currentTarget : null;
    let x: number, y: number;
    if (e instanceof MouseEvent && e.clientX) {
      x = e.clientX;
      y = e.clientY;
    } else {
      const r = menuOpener?.getBoundingClientRect();
      x = r ? r.left + r.width / 2 : 100;
      y = r ? r.top + r.height / 2 : 100;
    }
    keyMenu = { x: Math.min(x, window.innerWidth - 230), y: Math.min(y, window.innerHeight - 300), k, pages: false };
    await tick();
    focusMenuItem(0);
  }

  function menuItems(): HTMLButtonElement[] {
    return Array.from(document.querySelectorAll<HTMLButtonElement>(".key-menu button:not(:disabled)"));
  }

  function focusMenuItem(i: number) {
    const items = menuItems();
    if (items.length) items[(i + items.length) % items.length].focus();
  }

  function closeKeyMenu(restore = true) {
    keyMenu = null;
    if (restore && menuOpener?.isConnected) menuOpener.focus();
    menuOpener = null;
  }

  function menuKeydown(e: KeyboardEvent) {
    const items = menuItems();
    const at = items.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "ArrowDown") focusMenuItem(at + 1);
    else if (e.key === "ArrowUp") focusMenuItem(at - 1);
    else if (e.key === "Home") focusMenuItem(0);
    else if (e.key === "End") focusMenuItem(items.length - 1);
    else if (e.key === "Escape" || (e.key === "ArrowLeft" && keyMenu?.pages)) {
      if (keyMenu?.pages) showMenuPages(false);
      else closeKeyMenu();
    } else if (e.key === "Tab") closeKeyMenu(false);
    else return;
    e.preventDefault();
    e.stopPropagation();
  }

  async function showMenuPages(on: boolean) {
    if (!keyMenu) return;
    keyMenu = { ...keyMenu, pages: on };
    await tick();
    focusMenuItem(0);
  }

  /// Runs a menu item for the key the menu is on, then closes the menu.
  async function menuDo(f: (k: number) => Promise<unknown> | unknown) {
    const k = keyMenu!.k;
    closeKeyMenu();
    await f(k);
  }

  async function pressKey(k: number) {
    await call("press_key", { key: k });
    flash(`Pressed ${slotName(k).toLowerCase()}.`);
  }

  async function copyKey(k: number) {
    await call("copy_key", { key: k });
    if (error) return;
    clipboardFull = true;
    flash(`Copied ${keyInfos[k]?.name ?? slotName(k).toLowerCase()}. Paste it on any key.`);
  }

  async function pasteKey(k: number) {
    const had = keyInfos[k];
    await call("paste_key", { key: k });
    if (error) return;
    await afterKeyChange(k);
    if (had) showUndo({ text: `Pasted over ${had.name}.`, undo: async () => { await hostUndo(); } });
    else flash(`Pasted on ${slotName(k).toLowerCase()}.`);
  }

  /// The next free place of the same kind after `from` (wrapping round).
  function nextFree(from: number) {
    const same = allSlots.filter((k) => (k >= DIAL_BASE) === (from >= DIAL_BASE) && (k >= BUTTON_BASE && k < DIAL_BASE) === (from >= BUTTON_BASE && from < DIAL_BASE));
    const i = same.indexOf(from);
    const order = [...same.slice(i + 1), ...same.slice(0, i)];
    return order.find((k) => !keyInfos[k]);
  }

  async function duplicateKey(k: number) {
    const to = nextFree(k);
    if (to === undefined) {
      error = "There's no free key on this page to copy it to.";
      return;
    }
    await call("duplicate_key", { from: k, to });
    if (error) return;
    await afterKeyChange(to);
    flash(`Copied to ${slotName(to).toLowerCase()}.`);
  }

  async function moveToPage(k: number, target: NameId) {
    const to = await call<number>("move_key_to_page", { key: k, page: target.id });
    if (to === undefined) return;
    if (selected === k) await loadKey(null);
    refreshPage();
    showUndo({ text: `Moved to ${slotName(to).toLowerCase()} on ${target.name}.`, undo: async () => { await call("move_key_to_page", { key: to, page: layout!.page }); } });
  }

  async function afterKeyChange(k: number) {
    await refreshPage();
    if (selected === k) await loadKey(k);
    setTimeout(refreshPlugins, 500);
  }

  async function removeKeyAt(k: number) {
    const ki = keyInfos[k];
    const what = ki?.name ? `${ki.name} from ${slotName(k).toLowerCase()}` : slotName(k).toLowerCase();
    await call("clear_key", { key: k });
    if (selected === k) await loadKey(null);
    refreshPage();
    setTimeout(refreshPlugins, 500);
    showUndo({ text: `Removed ${what}.`, undo: async () => { await hostUndo(); } });
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
      refreshPage();
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
    await refreshPage();
    await loadKey(selected);
  }

  /// VSD Craft's plugins are only hidden. Agora's own are hidden first and
  /// go to the Recycle Bin once the Undo has gone.
  async function removePlugin(p: Plugin) {
    const own = p.source === "Agora";
    await call(own ? "hide_plugin" : "remove_plugin", { uuid: p.uuid });
    if (error) return;
    await refreshAll();
    showUndo({
      text: own ? `Deleted ${p.name}.` : `Hid ${p.name}.`,
      undo: hostUndo,
      commit: own
        ? async () => {
            await call("remove_plugin", { uuid: p.uuid });
            refreshAll();
          }
        : undefined,
    });
  }

  async function showPlugin(p: Plugin) {
    await call("show_plugin", { uuid: p.uuid });
    await refreshAll();
  }

  async function setVsdPlugins(on: boolean) {
    settings.use_vsd_craft_plugins = on;
    await call("set_use_vsd_craft_plugins", { on });
  }

  // ---- look ----

  type Theme = "system" | "light" | "dark";
  const themes: { id: Theme; label: string }[] = [
    { id: "system", label: "Like Windows" },
    { id: "light", label: "Light" },
    { id: "dark", label: "Dark" },
  ];
  // Applied before the settings arrive, so the window doesn't flash.
  let savedTheme: Theme = "system";
  try {
    savedTheme = (localStorage.getItem("agora.theme") as Theme) || "system";
  } catch {}
  let theme = $state<Theme>(savedTheme);

  function applyTheme(t: Theme) {
    if (t === "system") delete document.documentElement.dataset.theme;
    else document.documentElement.dataset.theme = t;
  }
  applyTheme(savedTheme);
  // Whether Windows is in dark mode, for "Like Windows".
  const darkQuery = window.matchMedia("(prefers-color-scheme: dark)");
  let systemDark = $state(darkQuery.matches);
  darkQuery.addEventListener("change", (e) => (systemDark = e.matches));
  const darkNow = $derived(theme === "dark" || (theme === "system" && systemDark));

  async function setTheme(t: Theme, save = true) {
    theme = t;
    applyTheme(t);
    try {
      localStorage.setItem("agora.theme", t);
    } catch {}
    if (save) await call("set_theme", { theme: t });
    else invoke("set_theme", { theme: t }).catch(() => {});
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
      refreshPage();
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

  const pendingPreviews = new Map<number, string | null>();
  let previewFrame = 0;
  function flushPreviews() {
    previewFrame = 0;
    const next = { ...previews };
    for (const [key, image] of pendingPreviews) {
      if (image) next[key] = image;
      else delete next[key];
    }
    pendingPreviews.clear();
    previews = next;
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
  const usedKeys = $derived(Object.keys(keyInfos).length);
  const runningPlugins = $derived(plugins.filter((p) => p.status === "running").length);
  const visiblePlugins = $derived(plugins.filter((p) => !p.hidden).length);

  onMount(() => {
    refreshStatus();
    loadPreviews();
    call<Settings>("get_settings").then((s) => {
      // The intro plays once, when the window is first seen: every time, or
      // ("update") on the first run and after an update. Not at all for
      // people who asked their system for less motion.
      let mode = s?.intro ?? "update";
      if (mode === "update") mode = s && s.intro_seen_version === s.app_version ? "off" : "agora";
      if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) mode = "off";
      if (s && s.intro_seen_version !== s.app_version) call("intro_played");
      if (mode !== "off") {
        const play = () => (introMode = mode);
        if (document.visibilityState === "visible") play();
        else {
          const once = () => {
            if (document.visibilityState === "visible") {
              document.removeEventListener("visibilitychange", once);
              play();
            }
          };
          document.addEventListener("visibilitychange", once);
        }
      }
      if (!s) return;
      settings = s;
      setTheme((s.theme as Theme) || "system", false);
    });
    call<Action[]>("list_actions").then((a) => (actions = a ?? []));
    refreshPlugins();
    refreshPage();
    invoke<VsdProfile[]>("list_vsd_profiles").then((v) => (vsdFound = v?.length ?? 0)).catch(() => {});
    const onKeydown = (e: KeyboardEvent) => {
      // Ctrl+Z puts back what was just removed, while Undo shows.
      if (toast && e.key.toLowerCase() === "z" && (e.ctrlKey || e.metaKey) && !(e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement)) {
        e.preventDefault();
        undoToast();
        return;
      }
      if (e.key === "Escape" && keyMenu) {
        closeKeyMenu();
        return;
      }
      const typing = e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement || e.target instanceof HTMLSelectElement || e.target instanceof HTMLIFrameElement;
      if (page === "keys" && selected !== null && !typing && !keyMenu) {
        const ctrl = e.ctrlKey || e.metaKey;
        if (e.key === "Delete" && keyInfos[selected]) {
          e.preventDefault();
          removeKey();
          return;
        }
        if (ctrl && e.key.toLowerCase() === "c" && keyInfos[selected] && !window.getSelection()?.toString()) {
          e.preventDefault();
          copyKey(selected);
          return;
        }
        if (ctrl && e.key.toLowerCase() === "v" && clipboardFull) {
          e.preventDefault();
          pasteKey(selected);
          return;
        }
      }
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
      // Faces come one key at a time; draw them together, once a frame.
      listen<Preview>("key-preview", (e) => {
        pendingPreviews.set(e.payload.key, e.payload.image);
        if (!previewFrame) previewFrame = requestAnimationFrame(flushPreviews);
      }),
      listen<Line>("plugin-status", (e) => {
        addLog(`${e.payload.plugin}: ${e.payload.message}`);
        refreshPlugins();
      }),
        listen<Line>("plugin-log", (e) => addLog(`${e.payload.plugin}: ${e.payload.message}`)),
      listen("page-changed", () => {
        if (performance.now() > ownChangeUntil) refreshPage();
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
    class:held={(keyInfos[k]?.hold ?? []).length > 0}
    onclick={() => (editable ? clickKey(k) : openKey(k))}
    ondblclick={() => editable && openFolder(k)}
    oncontextmenu={(e) => (editable ? openKeyMenu(e, k) : e.preventDefault())}
    onkeydown={(e) => {
      if (editable && (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10"))) openKeyMenu(e, k);
    }}
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
    aria-label={`${slotName(k)}, ${keyInfos[k]?.look?.title || keyInfos[k]?.name || "empty"}${(keyInfos[k]?.hold ?? []).length ? ", has a hold action" : ""}`}
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
    <input class="search" placeholder="Search actions" aria-label="Search actions" bind:value={search} />
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
          <button class="small" onclick={() => layoutOp("add-profile", undefined, `Profile ${layout!.profiles.length + 1}`)} title="Add a profile">New</button>
          <div class="more-wrap">
            <button class="small more" class:on={profileMenu || appsOpen || importOpen} onclick={() => (profileMenu = !profileMenu)}
              aria-haspopup="menu" aria-expanded={profileMenu} aria-label="More for this profile" title="More for this profile">⋯</button>
            {#if profileMenu}
              <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
              <div class="menu-backdrop" onclick={() => (profileMenu = false)}></div>
              <!-- svelte-ignore a11y_interactive_supports_focus -->
              <div class="key-menu drop-menu" role="menu" onkeydown={(e) => { if (e.key === "Escape") profileMenu = false; }}>
                <button role="menuitem" onclick={() => { profileMenu = false; startRename("profile", layout!.profiles.find((p) => p.id === layout!.profile)!); }}>Rename</button>
                <button role="menuitem" onclick={() => { profileMenu = false; toggleApps(); }}>Apps that bring it up{layout.apps.length ? ` (${layout.apps.length})` : ""}</button>
                <button role="menuitem" onclick={() => { profileMenu = false; toggleImport(); }}>Import from VSD Craft</button>
                {#if layout.profiles.length > 1}
                  <hr />
                  <button role="menuitem" class="danger" onclick={() => { profileMenu = false; removeWithUndo("profile", layout!.profile); }}>Delete profile</button>
                {/if}
              </div>
            {/if}
          </div>
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
      <nav class="tabs" aria-label="Pages">
        {#each layout.pages as p}
          {@const active = p.id === layout.top_page}
          {#if renaming?.kind === "page" && renaming.id === p.id}
            <!-- svelte-ignore a11y_autofocus -->
            <input class="rename tab-input" bind:value={renaming.name} autofocus aria-label="Page name"
              onkeydown={(e) => e.key === "Enter" && finishRename()} onblur={finishRename} />
          {:else}
            <span class="tab-wrap" class:active>
              <button
                class="tab"
                class:active
                aria-current={active ? "page" : undefined}
                onclick={() => !(active && layout!.path.length === 0) && layoutOp("go-to-page", p.id)}
                title={active ? p.name : `Show ${p.name}`}>{p.name}</button
              >
              {#if active && layout.path.length === 0}
                <button class="tab-edit" onclick={() => startRename("page", p)} aria-label="Rename {p.name}" title="Rename this page">
                  <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 20h4L19 9l-4-4L4 16zM14 6l4 4" /></svg>
                </button>
                {#if layout.pages.length > 1}
                  <button class="tab-edit bad" onclick={() => removeWithUndo("page", p.id)} aria-label="Delete {p.name}"
                    title="Delete this page and the folders on it. You can undo it.">
                    <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 7h14M10 7V4h4v3M7 7l1 13h8l1-13" /></svg>
                  </button>
                {/if}
              {/if}
            </span>
          {/if}
        {/each}
        <button class="tab add" onclick={() => layoutOp("add-page")} title="Add a page">+ Page</button>
      </nav>
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
      <p class="muted small">Its keys get their own page, with a Back key on key 1.</p>
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
          <option value="next">Next profile</option>
          <option value="previous">Previous profile</option>
          {#each layout?.profiles ?? [] as p}<option value={p.id}>{p.name}</option>{/each}
        </select>
      </label>
    {:else if action === "open"}
      <label>App, file or folder
        <span class="row grow"><input value={st.path ?? ""} placeholder="C:\Program Files\..." onchange={(e) => { save({ path: e.currentTarget.value }); if (st === info?.settings) useAppIcon(e.currentTarget.value); }} />
          <button onclick={() => pickProgram(save, st === info?.settings)}>File…</button><button onclick={() => pickFolderPath(save)}>Folder…</button></span>
      </label>
      <label>Arguments <span class="muted small">(programs only)</span><input value={st.args ?? ""} onchange={(e) => save({ args: e.currentTarget.value })} /></label>
    {:else if action === "website"}
      <label>Web address <input value={st.url ?? ""} placeholder="https://" onchange={(e) => save({ url: e.currentTarget.value })} /></label>
    {:else if action === "hotkey"}
      <label>Keys
        <input class="hotkey" value={st.keys ?? ""} placeholder="Click here, then press the keys" readonly aria-describedby="hotkey-help" onkeydown={(e) => recordHotkey(e, save)} />
      </label>
      <p class="muted small" id="hotkey-help">Press the combination, such as Ctrl+Shift+M. Backspace clears it.</p>
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
    {:else if action === "quick-control"}
      <label>Each press
        <select value={st.what ?? "show-desktop"} onchange={(e) => save({ what: e.currentTarget.value })}>
          <option value="show-desktop">Show the desktop</option><option value="lock">Lock the computer</option><option value="sleep">Put the computer to sleep</option>
          <option value="screen-off">Turn the screens off</option><option value="mic-mute">Mute or unmute the microphone</option><option value="switch-screen">Switch screens (projector menu)</option>
        </select>
      </label>
    {:else if action === "quick-tool"}
      <label>Opens
        <select value={st.tool ?? "calculator"} onchange={(e) => save({ tool: e.currentTarget.value })}>
          <option value="calculator">Calculator</option><option value="emoji">Emoji panel</option><option value="task-manager">Task Manager</option>
          <option value="search">Windows search</option><option value="notifications">Notifications</option><option value="settings">Windows Settings</option>
          <option value="control-panel">Control Panel</option><option value="file-explorer">File Explorer</option><option value="snip">Screenshot (snip)</option>
          <option value="clipboard">Clipboard history</option><option value="run">Run box</option><option value="mail">New email</option>
        </select>
      </label>
    {:else if action === "close-window"}
      <p class="muted small">Closes whichever window is in front, like clicking its ×. Apps with unsaved work still ask first.</p>
    {:else if action === "dock-sleep"}
      <p class="muted small">Turns the dock's screen and light ring off. Press any key to wake it; that press doesn't run the key.</p>
    {:else if action === "mouse"}
      <label>Each press
        <select value={st.mode ?? "click"} onchange={(e) => save({ mode: e.currentTarget.value })}>
          <option value="click">Click</option><option value="scroll-up">Scroll up</option><option value="scroll-down">Scroll down</option>
          <option value="scroll-left">Scroll left</option><option value="scroll-right">Scroll right</option><option value="move">Move the pointer</option>
        </select>
      </label>
      {#if (st.mode ?? "click") === "click"}
        <label>Button
          <select value={st.button ?? "left"} onchange={(e) => save({ button: e.currentTarget.value })}>
            <option value="left">Left</option><option value="right">Right</option><option value="middle">Middle</option>
          </select>
        </label>
        <label class="check"><input type="checkbox" checked={!!st.double} onchange={(e) => save({ double: e.currentTarget.checked })} /> Double-click</label>
      {:else if st.mode === "move"}
        <span class="row"><label>Across <input type="number" value={st.x ?? 0} onchange={(e) => save({ x: Number(e.currentTarget.value) })} /></label>
          <label>Down <input type="number" value={st.y ?? 0} onchange={(e) => save({ y: Number(e.currentTarget.value) })} /></label></span>
        <label class="check"><input type="checkbox" checked={st.relative ?? true} onchange={(e) => save({ relative: e.currentTarget.checked })} /> From where the pointer is now (otherwise from the top-left of the screen)</label>
      {:else}
        <label>Notches <input type="number" min="1" max="50" value={st.amount ?? 3} onchange={(e) => save({ amount: Number(e.currentTarget.value) })} /></label>
      {/if}
    {:else if action === "browser"}
      <label>Each press
        <select value={st.what ?? "back"} onchange={(e) => save({ what: e.currentTarget.value })}>
          <option value="back">Back</option><option value="forward">Forward</option><option value="refresh">Refresh</option><option value="stop">Stop loading</option>
          <option value="home">Home page</option><option value="new-tab">New tab</option><option value="close-tab">Close tab</option>
          <option value="reopen-tab">Reopen closed tab</option><option value="next-tab">Next tab</option><option value="previous-tab">Previous tab</option>
        </select>
      </label>
    {:else if action === "password"}
      <label>Name <input value={st.name ?? ""} placeholder="Work email" onchange={(e) => save({ name: e.currentTarget.value })} /></label>
      {#if (st.name ?? "").trim()}
        {#await pwSaved(st.name, pwTick) then saved}
          <p class="muted small">{saved ? "A password is saved under this name." : "No password saved under this name yet."}</p>
        {/await}
        <span class="row grow"><input type="password" autocomplete="off" placeholder="Type the password" bind:value={pwDraft} />
          <button onclick={async () => { await call("save_password", { name: st.name, secret: pwDraft }); pwDraft = ""; pwTick++; }} disabled={!pwDraft}>Save</button>
          <button onclick={async () => { await call("forget_password", { name: st.name }); pwTick++; }}>Forget</button></span>
        <p class="muted small">Kept in Windows Credential Manager under "Agora/{st.name}", never in Agora's own files.</p>
      {/if}
      <label class="check"><input type="checkbox" checked={!!st.enter} onchange={(e) => save({ enter: e.currentTarget.checked })} /> Press Enter after</label>
    {:else if action === "sound"}
      <label>Each press
        <select value={st.mode ?? "play"} onchange={(e) => save({ mode: e.currentTarget.value })}>
          <option value="play">Play a sound</option><option value="stop">Stop all sounds</option>
        </select>
      </label>
      {#if (st.mode ?? "play") === "play"}
        <label>Sound file
          <span class="row grow"><input value={st.path ?? ""} placeholder="C:\Sounds\horn.mp3" onchange={(e) => save({ path: e.currentTarget.value })} />
            <button onclick={() => pickSound(save)}>File…</button></span>
        </label>
        <label class="slider"><span>Volume <b>{st.volume ?? 100}%</b></span>
          <input type="range" min="0" max="100" value={st.volume ?? 100} style="--fill: {st.volume ?? 100}%" onchange={(e) => save({ volume: Number(e.currentTarget.value) })} />
        </label>
      {/if}
    {:else if action === "udp"}
      <span class="row"><label>Address <input value={st.host ?? ""} placeholder="192.168.1.50" onchange={(e) => save({ host: e.currentTarget.value })} /></label>
        <label>Port <input type="number" min="1" max="65535" value={st.port || ""} onchange={(e) => save({ port: Number(e.currentTarget.value) })} /></label></span>
      <label>Message <input value={st.message ?? ""} onchange={(e) => save({ message: e.currentTarget.value })} /></label>
      <label class="check"><input type="checkbox" checked={!!st.hex} onchange={(e) => save({ hex: e.currentTarget.checked })} /> The message is hex bytes (like 01 ff 7a)</label>
    {:else if action === "disk"}
      <label>Drive <input value={st.drive ?? "C:"} maxlength="3" onchange={(e) => save({ drive: e.currentTarget.value })} /></label>
      <label>Shows
        <select value={st.show ?? "space"} onchange={(e) => save({ show: e.currentTarget.value })}>
          <option value="space">How full it is</option><option value="busy">How busy it is</option>
        </select>
      </label>
    {:else if action === "ip"}
      <label>Shows
        <select value={st.which ?? "public"} onchange={(e) => save({ which: e.currentTarget.value })}>
          <option value="public">Public address (what websites see)</option><option value="local">Local address (on your network)</option>
        </select>
      </label>
      <p class="muted small">The public address is looked up every 15 minutes through OpenDNS; press the key to look again.</p>
    {:else if ["cpu", "memory", "gpu", "gpu-memory"].includes(action)}
      <p class="muted small">Updates every 2 seconds while the key is on the dock.</p>
    {:else if action === "hotkey-switch"}
      <label>First press
        <input class="hotkey" value={st.keys ?? ""} placeholder="Click here, then press the keys" readonly onkeydown={(e) => recordHotkey(e, (p) => save({ keys: p.keys }))} />
      </label>
      <label>Second press
        <input class="hotkey" value={st.keys_b ?? ""} placeholder="Click here, then press the keys" readonly onkeydown={(e) => recordHotkey(e, (p) => save({ keys_b: p.keys }))} />
      </label>
      <p class="muted small">The key shows which one comes next. Backspace clears a box.</p>
    {:else if action === "random"}
      {@render stepsEditor("steps", "Each press runs one of these, picked at random")}
    {:else if action === "audio-output"}
      <label>Each press
        <select value={st.mode ?? "cycle"} onchange={(e) => save({ mode: e.currentTarget.value })}>
          <option value="cycle">Switch to the next output</option><option value="set">Switch to one output</option>
        </select>
      </label>
      {#if st.mode === "set"}
        {#await audioOutputs() then outs}
          <label>Output
            <select value={st.device ?? ""} onchange={(e) => { const o = outs.find((x) => x.id === e.currentTarget.value); save({ device: e.currentTarget.value, device_name: o?.name ?? "" }); }}>
              <option value="" disabled>Choose an output</option>
              {#if st.device && !outs.some((o) => o.id === st.device)}<option value={st.device}>{st.device_name || "Saved output"} (not plugged in)</option>{/if}
              {#each outs as o}<option value={o.id}>{o.name}{o.default ? " (in use)" : ""}</option>{/each}
            </select>
          </label>
        {/await}
      {/if}
    {:else if action === "app-volume"}
      <label>App
        <input value={st.app ?? ""} list="audio-apps" placeholder="spotify.exe" onfocus={loadAudioApps} onchange={(e) => save({ app: e.currentTarget.value })} />
        <datalist id="audio-apps">{#each audioApps as a}<option value={a}></option>{/each}</datalist>
      </label>
      <p class="muted small">Apps playing sound now are suggested. The app needs to have played sound since it started.</p>
      <label>Each press
        <select value={st.what ?? "up"} onchange={(e) => save({ what: e.currentTarget.value })}>
          <option value="up">Louder</option><option value="down">Quieter</option><option value="mute">Mute or unmute</option>
        </select>
      </label>
      {#if (st.what ?? "up") !== "mute"}
        <label class="slider"><span>Step <b>{st.step ?? 10}%</b></span>
          <input type="range" min="1" max="50" value={st.step ?? 10} style="--fill: {((st.step ?? 10) / 50) * 100}%" onchange={(e) => save({ step: Number(e.currentTarget.value) })} />
        </label>
      {/if}
    {:else if action === "web-request"}
      <span class="row"><label>Method
        <select value={st.method ?? "POST"} onchange={(e) => save({ method: e.currentTarget.value })}>
          {#each ["GET", "POST", "PUT", "PATCH", "DELETE"] as m}<option value={m}>{m}</option>{/each}
        </select></label></span>
      <label>Web address <input value={st.url ?? ""} placeholder="http://homeassistant.local:8123/api/webhook/lights" onchange={(e) => save({ url: e.currentTarget.value })} /></label>
      {#if (st.method ?? "POST") !== "GET"}
        <label>Body <textarea rows="3" value={st.body ?? ""} placeholder={'{"on": true}'} onchange={(e) => save({ body: e.currentTarget.value })}></textarea></label>
        <label>Content type <input value={st.content_type ?? "application/json"} onchange={(e) => save({ content_type: e.currentTarget.value })} /></label>
      {/if}
      <p class="muted small">The key shows a tick when the request works, or the reason when it doesn't.</p>
    {:else if action === "run-command"}
      <label>PowerShell command <textarea rows="3" class="mono" value={st.command ?? ""} placeholder="(Get-Date).ToString('HH:mm')" onchange={(e) => save({ command: e.currentTarget.value })}></textarea></label>
      <label class="check"><input type="checkbox" checked={st.show_output ?? true} onchange={(e) => save({ show_output: e.currentTarget.checked })} /> Show what it prints on the key</label>
      <p class="muted small">Runs hidden, as you, and stops after a minute.</p>
    {:else if action === "snippets"}
      <label>Snippets, one per line
        <textarea rows="5" value={(st.snippets ?? []).join("\n")} placeholder={"Thanks, speak soon!\nmy.name@example.com"} onchange={(e) => save({ snippets: e.currentTarget.value.split("\n").filter((l) => l.trim()) })}></textarea>
      </label>
      <label class="check"><input type="checkbox" checked={!!st.enter} onchange={(e) => save({ enter: e.currentTarget.checked })} /> Press Enter after</label>
      <p class="muted small">A press shows the list beside the mouse. Pick one (or press its number) and it's typed where you were.</p>
    {:else if action === "window"}
      <label>Each press
        <select value={st.what ?? "next-screen"} onchange={(e) => save({ what: e.currentTarget.value })}>
          <option value="next-screen">Move to the next screen</option><option value="snap-left">Snap to the left half</option>
          <option value="snap-right">Snap to the right half</option><option value="snap-top">Snap to the top half</option>
          <option value="snap-bottom">Snap to the bottom half</option><option value="maximize">Maximise or restore</option>
          <option value="minimize">Minimise</option><option value="on-top">Keep on top, or stop</option>
        </select>
      </label>
      <p class="muted small">Works on the window in front.</p>
    {:else if action === "clock"}
      <label>Time
        <select value={st.format ?? "24h"} onchange={(e) => save({ format: e.currentTarget.value })}>
          <option value="24h">24-hour (14:05)</option><option value="12h">12-hour (2:05 PM)</option>
        </select>
      </label>
      <label class="check"><input type="checkbox" checked={!!st.seconds} onchange={(e) => save({ seconds: e.currentTarget.checked })} /> Show seconds</label>
    {:else if action === "timer"}
      <span class="row"><label>Minutes <input type="number" min="0" max="5999" value={st.minutes ?? 5} onchange={(e) => save({ minutes: Number(e.currentTarget.value) })} /></label>
        <label>Seconds <input type="number" min="0" max="59" value={st.seconds ?? 0} onchange={(e) => save({ seconds: Number(e.currentTarget.value) })} /></label></span>
      <label>Sound when it ends <span class="muted small">(optional)</span>
        <span class="row grow"><input value={st.sound ?? ""} placeholder="C:\Sounds\bell.mp3" onchange={(e) => save({ sound: e.currentTarget.value })} />
          <button onclick={async () => { const p = await pickFile("Choose a sound", "Sounds", ["mp3", "wav", "wma", "m4a", "aac"]); if (p) save({ sound: p }); }}>File…</button></span>
      </label>
      <p class="muted small">Press to start or pause. Hold to reset. When it ends, the next press resets it.</p>
    {:else if action === "stopwatch"}
      <p class="muted small">Press to start or pause. Hold to reset.</p>
    {:else if action === "counter"}
      <label>Name <span class="muted small">(shown above the number)</span><input value={st.name ?? ""} placeholder="Count" onchange={(e) => save({ name: e.currentTarget.value })} /></label>
      <span class="row"><label>Add each press <input type="number" value={st.step ?? 1} onchange={(e) => save({ step: Number(e.currentTarget.value) })} /></label>
        <label>Count now <input type="number" value={st.count ?? 0} onchange={(e) => save({ count: Number(e.currentTarget.value) })} /></label></span>
      <p class="muted small">Hold the key to put it back to 0.</p>
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
  {@const list = (field === HOLD ? (info?.hold ?? []) : (info?.settings[field] ?? [])) as Step[]}
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
          {:else if p.id === "settings"}<svg viewBox="0 0 24 24"><path d="M4 7h10M18 7h2M4 17h2M10 17h10" /><circle cx="16" cy="7" r="2" /><circle cx="8" cy="17" r="2" /></svg>
          {:else}<svg viewBox="0 0 24 24"><ellipse cx="12" cy="12" rx="9" ry="6" /><circle cx="12" cy="12" r="2" /></svg>{/if}
        </span>
        {p.label}
      </button>
    {/each}
    <div class="rail-foot">
      <button class="theme-toggle" onclick={() => setTheme(darkNow ? "light" : "dark")}
        aria-label={darkNow ? "Switch to the light theme" : "Switch to the dark theme"} title={darkNow ? "Light theme" : "Dark theme"}>
        {#if darkNow}
          <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="4" /><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" /></svg>
        {:else}
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z" /></svg>
        {/if}
      </button>
      <span class="pill" class:off={!status.connected}>{status.connected ? status.model ?? "Connected" : "No dock"}</span>
    </div>
  </nav>

  <main class="page-{page}">
    {#if page === "home"}
      <section class="hero">
        <div class="meander"></div>
        <div class="hero-body">
          <div>
            <h1>{status.model_name}</h1>
            <p class="tagline">Keys, pages and plugins for your Stream Dock.</p>
            <span class="pill big" class:off={!status.connected}>{status.message || (status.connected ? "Connected" : "Not connected")}</span>
            {#if !status.connected}
              <p class="muted hint">Agora keeps looking and connects by itself as soon as the dock is free.</p>
            {/if}
            {#if layout}
              <dl class="now">
                <dt>Profile</dt><dd>{layout.profiles.find((p) => p.id === layout!.profile)?.name}</dd>
                <dt>Page</dt><dd>{pageName}</dd>
              </dl>
            {/if}
          </div>
          <div class="hero-dock">
            {@render keyGrid(false)}
          </div>
        </div>
      </section>

      <div class="cards">
        <button class="card" onclick={() => (page = "keys")}>
          <span class="card-num">{usedKeys}<small>/{allSlots.length}</small></span>
          <span class="card-label">{allSlots.length > keyCount ? "Keys, buttons and knobs set up" : "Keys set up"} on this page</span>
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

      {#if vsdFound > 0 && !vsdOfferHidden}
        <section class="offer">
          <div>
            <h2>Bring your keys over from VSD Craft</h2>
            <p class="muted">Agora found {vsdFound} VSD Craft {vsdFound === 1 ? "profile" : "profiles"} for this dock. Importing copies them into Agora and leaves VSD Craft's files as they are.</p>
          </div>
          <div class="row">
            <button class="primary" onclick={startImport}>Import from VSD Craft</button>
            <button onclick={hideVsdOffer}>Not now</button>
          </div>
        </section>
      {/if}
    {:else if page === "keys"}
      <div class="keys-page">
        <section class="device">
          <div class="meander"></div>
          {@render pageBar()}
          {@render keyGrid(true)}
          {#if Object.keys(keyInfos).length > 0}
            <div class="key-list">
              <h3>On this page <span class="muted small">{Object.keys(keyInfos).length} of {allSlots.length}</span></h3>
              {#each Object.values(keyInfos) as ki (ki.key)}
                {@const hold = ki.hold ?? []}
                <div class="key-row-wrap">
                  <button class="key-row" class:current={selected === ki.key} onclick={() => loadKey(ki.key)} oncontextmenu={(e) => openKeyMenu(e, ki.key)}
                    onkeydown={(e) => { if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) openKeyMenu(e, ki.key); }}>
                    {#if previews[ki.key]}<img src={previews[ki.key]} alt="" />{:else}<span class="noicon"></span>{/if}
                    <span class="key-row-text"><b><span class="k">{slotName(ki.key)}</span> · {ki.name}</b><span class="muted small">{ki.plugin_name}{#if hold.length}<span class="hold-note">{" · "}hold: {stepName(hold[0].action)}{hold.length > 1 ? ` +${hold.length - 1}` : ""}</span>{/if}</span></span>
                  </button>
                  {#if ki.builtin && ki.action === "folder"}
                    <button class="small" onclick={() => openFolder(ki.key)} aria-label="Open folder {ki.settings.name ?? ''}">Open</button>
                  {/if}
                </div>
              {/each}
            </div>
          {/if}
        </section>

        <section class="side">
          <div class="triglyph"></div>
          {#if selected === null}
            <div class="side-head">
              <div>
                <h2>Action library</h2>
                <div class="muted small">{Object.keys(keyInfos).length} of {allSlots.length} {allSlots.length > keyCount ? "keys, buttons and knobs" : "keys"} on {pageName || "this page"} set up.</div>
              </div>
            </div>
            <p class="muted hint">Drag an action onto a key{status.buttons ? " or button" : ""}{status.dials ? " or knob" : ""}, or click it to use the first free one. Drag keys onto each other to move or swap them; right-click a key for more.</p>
            {@render library()}
          {:else if picking}
            <div class="editor-head">
              <button class="link back" onclick={() => (info ? (picking = false) : loadKey(null))}>{info ? "← Back to the key" : "← All keys"} <span class="muted">(Esc)</span></button>
              <h2>Pick an action for <span class="k">{slotName(selected).toLowerCase()}</span></h2>
            </div>
            <p class="muted hint">{isDial(selected) ? "Knobs take Knob, or plugin actions made for knobs." : `Click an action to put it on ${slotName(selected).toLowerCase()}, or drag it onto any key.`}</p>
            {#if clipboardFull}<div class="row"><button class="small" onclick={() => pasteKey(selected!)}>Paste the copied key here</button></div>{/if}
            {@render library()}
          {:else if info}
            {@const hold = info.hold ?? []}
            <div class="editor-head">
              <button class="link back" onclick={() => loadKey(null)}>← All keys <span class="muted">(Esc)</span></button>
              <div>
                <h2><span class="k">{slotName(selected)}</span> · <span class="item">{info.name}</span></h2>
                <div class="muted small">{info.plugin_name}</div>
              </div>
              <div class="row editor-actions">
                {#if info.builtin && info.action === "folder"}<button onclick={() => openFolder(selected!)}>Open folder</button>{/if}
                <button onclick={() => (picking = true)}>Change action</button>
                <button onclick={() => pressKey(selected!)} title="Run this key's action now, as if you tapped it">Press now</button>
                <button class="danger push" onclick={removeKey} title="Remove this key's action (Delete). You can undo it.">Remove</button>
              </div>
            </div>
            {#if info.builtin}
              {@render actionForm(info.action, info.settings, saveSettings)}
            {:else if info.settings_url}
              {#key info.settings_url}
                <iframe class="pi" src={info.settings_url} title="Settings for {info.name}"></iframe>
              {/key}
            {:else}
              <p class="muted">This action has no settings.</p>
            {/if}
            {#if selected !== null && !isDial(selected) && !(info.builtin && ["dial"].includes(info.action))}
              <details class="look" open={hold.length > 0}>
                <summary>When held{hold.length ? ` · ${hold.length} step${hold.length > 1 ? "s" : ""}` : ""}</summary>
                <p class="muted small">Hold the key for {((settings.hold_ms ?? 500) / 1000).toLocaleString()} s to run these instead of its action. A quick press still runs the action.{info.builtin && ["timer", "stopwatch", "counter"].includes(info.action) ? " With nothing here, holding resets it." : ""} <button class="link" onclick={() => (page = "settings")}>Change the hold time</button></p>
                {@render stepsEditor(HOLD, "Steps when held")}
              </details>
            {/if}
            {#if selected !== null && (selected < BUTTON_BASE || (isDial(selected) && status.strip))}
              {@render lookEditor(info)}
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
            <span class="muted small">Lists the plugins you installed for VSD Craft in Agora too. Agora only reads them and never changes or deletes them.</span>
          </span>
        </label>
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
                <button class="small danger" onclick={() => removePlugin(p)}
                  title="Moves the plugin to the Recycle Bin and clears its keys. You can undo it for a few seconds.">Delete</button>
              {:else}
                <button class="small" onclick={() => removePlugin(p)}
                  title="VSD Craft's plugins are never deleted by Agora. Hiding takes it out of the action list and clears its keys.">Hide</button>
              {/if}
            </div>
          </div>
        {/each}
      </section>
      <section class="log">
        <h2>Activity</h2>
        {@render activity(200)}
      </section>
    {:else if page === "dock"}
      <div class="dock-page">
        <section class="screen-card">
          <h2>Screen</h2>
          <p class="muted small model-line">{status.model_name}{status.connected ? "" : " (last connected)"}{#if !status.tested}. Agora hasn't been tried on this model yet. <button class="link" onclick={() => call("open_link", { which: "issues" })}>Tell us what works and what doesn't</button>.{/if}</p>
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
          <div class="form saver">
            <div class="field-label" id="saver-label">Screen saver</div>
            <div class="saver-row">
              <select aria-labelledby="saver-label" value={settings.saver ?? "off"} onchange={(e) => saveSaver({ saver: e.currentTarget.value })}>
                {#each savers as sv}<option value={sv.id}>{sv.name}</option>{/each}
              </select>
              {#if (settings.saver ?? "off") !== "off"}
                <span class="after">after</span>
                <input type="number" min="1" max="240" aria-label="Minutes unused before it starts" value={settings.saver_minutes ?? 10} onchange={(e) => saveSaver({ saver_minutes: Number(e.currentTarget.value) })} />
                <span class="after">min</span>
              {/if}
            </div>
            {#if settings.saver === "gif"}
              <label>GIF
                <span class="row grow"><input value={settings.saver_gif ?? ""} placeholder="C:\Pictures\loop.gif" onchange={(e) => saveSaver({ saver_gif: e.currentTarget.value })} />
                  <button onclick={async () => { const p = await pickFile("Choose a GIF", "GIF", ["gif"]); if (p) saveSaver({ saver_gif: p }); }}>File…</button></span>
              </label>
              <p class="muted small">It's stretched across all the keys, cropped to fit.</p>
            {/if}
            {#if (settings.saver ?? "off") !== "off"}
              <p class="muted small">Any press on the dock stops it; that press doesn't run the key. <button class="link" onclick={() => call("preview_screen_saver", { mode: settings.saver, gif: settings.saver_gif ?? "" })}>Try it on the dock</button></p>
            {/if}
          </div>
          <label class="switch-row">
            <input type="checkbox" role="switch" checked={settings.dock_intro ?? true} onchange={(e) => saveIntro({ dock_intro: e.currentTarget.checked })} />
            <span>
              Build the temple on the dock when it connects
              <span class="muted small">About two seconds, then your keys come back. <button class="link" onclick={() => call("preview_screen_saver", { mode: "intro", gif: "" })}>Try it</button></span>
            </span>
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
            <button class:active={!paint} aria-pressed={!paint} onclick={() => (paint = false)}>Whole ring</button>
            <button class:active={paint} aria-pressed={paint} onclick={() => { paint = true; brush = settings.ring_color; }}>Paint lights</button>
          </div>
          {#if paint}<p class="muted small">Pick a colour, then click or drag over the lights.</p>{/if}
          {/if}
          <div class="swatches" role="group" aria-label="Ring colour">
            {#each ringSwatches as sw}
              <button
                class="swatch-btn"
                class:chosen={paint ? brush === sw.color : settings.ring_on && !settings.ring_colors.length && settings.ring_color.toLowerCase() === sw.color}
                aria-pressed={paint ? brush === sw.color : settings.ring_on && !settings.ring_colors.length && settings.ring_color.toLowerCase() === sw.color}
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
        </section>
        {/if}

      </div>
    {:else}
      <div class="settings-page">
        <section>
          <h2>Look</h2>
          <div class="theme-row">
            <span>Theme</span>
            <div class="seg" role="group" aria-label="Light or dark">
              {#each themes as t}
                <button class:active={theme === t.id} aria-pressed={theme === t.id} onclick={() => setTheme(t.id)}>{t.label}</button>
              {/each}
            </div>
          </div>
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

        <section>
          <h2>Intro</h2>
          <div class="form">
            <label>When Agora opens
              <select value={settings.intro ?? "update"} onchange={(e) => saveIntro({ intro: e.currentTarget.value })}>
                <option value="update">The temple, after an update</option>
                <option value="agora">The temple, every time</option>
                <option value="video">Your own video</option>
                <option value="off">Nothing</option>
              </select>
            </label>
            {#if settings.intro === "video"}
              <label>Video
                <span class="row grow"><input value={settings.intro_video ?? ""} placeholder="C:\Videos\intro.mp4" onchange={(e) => saveIntro({ intro_video: e.currentTarget.value })} />
                  <button onclick={async () => { const p = await pickFile("Choose a video", "Videos", ["mp4", "webm", "m4v"]); if (p) saveIntro({ intro_video: p }); }}>File…</button></span>
              </label>
              <p class="muted small">MP4 or WebM, up to 300 MB. A click or any key skips it.</p>
            {/if}
            <p class="muted small">It's skipped when Windows is set to show fewer animations.</p>
            <div class="row"><button onclick={() => (introMode = settings.intro === "video" ? "video" : "agora")}>Play it now</button></div>
          </div>
        </section>

        <section>
          <h2>Keys</h2>
          <label class="slider">
            <span>Hold time <b>{((settings.hold_ms ?? 500) / 1000).toLocaleString()} s</b></span>
            <input type="range" min="300" max="1000" step="50" bind:value={settings.hold_ms}
              style="--fill: {(((settings.hold_ms ?? 500) - 300) / 700) * 100}%"
              onchange={() => call("set_hold_time", { ms: settings.hold_ms ?? 500 })} />
          </label>
          <p class="muted small">How long to keep a key down for its "When held" steps. Shorter feels quicker; longer avoids running them by accident.</p>
        </section>
      </div>
    {/if}
    {#if introMode}
      <Intro mode={introMode} ondone={() => (introMode = null)} />
    {/if}
    {#if keyMenu}
      {@const mk = keyMenu.k}
      {@const has = !!keyInfos[mk]}
      {@const isFolder = keyInfos[mk]?.builtin && keyInfos[mk]?.action === "folder"}
      <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
      <div class="menu-backdrop" onclick={() => closeKeyMenu(false)} oncontextmenu={(e) => { e.preventDefault(); closeKeyMenu(false); }}></div>
      <!-- svelte-ignore a11y_interactive_supports_focus -->
      <div class="key-menu" role="menu" aria-label="{slotName(mk)}" style="left: {keyMenu.x}px; top: {keyMenu.y}px" onkeydown={menuKeydown}>
        {#if keyMenu.pages}
          <button role="menuitem" class="sub-back" onclick={() => showMenuPages(false)}>‹ Move to page</button>
          {#each (layout?.pages ?? []).filter((g) => g.id !== layout?.page) as g}
            <button role="menuitem" onclick={() => menuDo((k) => moveToPage(k, g))}>{g.name}</button>
          {/each}
        {:else if has}
          <button role="menuitem" onclick={() => menuDo(loadKey)}>Edit key</button>
          <button role="menuitem" onclick={() => menuDo(pressKey)}>Press now</button>
          <hr />
          <button role="menuitem" disabled={isFolder} onclick={() => menuDo(copyKey)}>Copy <kbd>Ctrl+C</kbd></button>
          <button role="menuitem" disabled={!clipboardFull} onclick={() => menuDo(pasteKey)}>Paste <kbd>Ctrl+V</kbd></button>
          <button role="menuitem" disabled={isFolder || nextFree(mk) === undefined} onclick={() => menuDo(duplicateKey)}>Duplicate to next free key</button>
          {#if (layout?.pages.length ?? 0) > 1 || (layout?.path.length ?? 0) > 0}
            <button role="menuitem" aria-haspopup="menu" onclick={() => showMenuPages(true)}
              onkeydown={(e) => { if (e.key === "ArrowRight") { e.preventDefault(); showMenuPages(true); } }}>Move to page <span class="sub">›</span></button>
          {/if}
          <hr />
          <button role="menuitem" class="danger" onclick={() => menuDo(removeKeyAt)}>Remove key <kbd>Del</kbd></button>
        {:else}
          <button role="menuitem" onclick={() => menuDo(loadKey)}>Pick an action</button>
          <button role="menuitem" disabled={!clipboardFull} onclick={() => menuDo(pasteKey)}>Paste <kbd>Ctrl+V</kbd></button>
        {/if}
      </div>
    {/if}
    <div class="statusbar" class:empty={!error && !toast && !notice}>
      {#if error}
        <div class="error-line" role="alert">
          <span>{error}</span>
          <button class="small" onclick={() => (error = "")} aria-label="Dismiss">×</button>
        </div>
      {/if}
      <div class="status-line" role="status" aria-live="polite">
        {#if toast}
          <span>{toast.text}</span>
          <button class="small" onclick={undoToast}>Undo</button>
        {:else if notice}
          <span class="muted">{notice}</span>
        {/if}
      </div>
    </div>
  </main>
</div>
