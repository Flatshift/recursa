import './app.css';
import { mount } from 'svelte';
import App from './App.svelte';

const target = document.getElementById('app')!;
// The iOS shell sets this when the engine couldn't start: show why, since there's no app to run.
const fatal = (window as { __RECURSA_FATAL__?: { message: string; detail: string } }).__RECURSA_FATAL__;
if (fatal) {
  const h = document.createElement('h1');
  const p = document.createElement('p');
  h.textContent = fatal.message;
  p.textContent = fatal.detail;
  target.className = 'fatal';
  target.append(h, p);
} else {
  mount(App, { target });
}
