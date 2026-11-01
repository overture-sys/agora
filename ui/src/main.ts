import { mount } from "svelte";
import App from "./App.svelte";
import Snippets from "./Snippets.svelte";
import "./app.css";

// The Snippets pop-up is the same page in its own small window.
const target = document.getElementById("app")!;
const snippets = (window as unknown as { __agoraSnippets?: boolean }).__agoraSnippets === true;
export default snippets ? mount(Snippets, { target }) : mount(App, { target });
