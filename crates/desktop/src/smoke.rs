//! Debug builds only. `RECURSA_SMOKE=1 target/debug/recursa-desktop` drives the real UI in
//! the real WebView: boot over IPC, a short live run followed through the store's channel,
//! the timing floor at startup and with the window open, an app-drawn tooltip, then a menu command. It prints each step and exits 0 on PASS, 1 on FAIL.

use tauri::{AppHandle, Emitter, Listener, WebviewWindow};

pub fn enabled() -> bool {
    std::env::var_os("RECURSA_SMOKE").is_some()
}

pub fn install(app: &AppHandle) {
    let handle = app.clone();
    app.listen("smoke", move |e| {
        let line: String = serde_json::from_str(e.payload()).unwrap_or_default();
        eprintln!("[smoke] {line}");
        if line == "menu?" {
            let _ = handle.emit_to(crate::shell::MAIN, "recursa:menu", "go:/reliability");
        } else if line.starts_with("PASS") {
            handle.exit(0);
        } else if line.starts_with("FAIL") {
            handle.exit(1);
        }
    });
}

pub fn run(w: &WebviewWindow) {
    let _ = w.eval(SCRIPT);
}

const SCRIPT: &str = r#"(async () => {
  const T = window.__TAURI_INTERNALS__;
  const say = (line) => T.invoke('plugin:event|emit', { event: 'smoke', payload: line });
  const text = () => document.body.innerText;
  const until = async (f, ms) => {
    for (const end = Date.now() + ms; Date.now() < end; await new Promise((r) => setTimeout(r, 150))) {
      const v = f();
      if (v) return v;
    }
    return null;
  };
  try {
    const failed = () => /can.t reach its engine/.test(text());
    const booted = await until(() => text().includes('Test plan') || failed(), 15000);
    if (!booted || failed()) return say('FAIL boot: ' + text().slice(0, 300));
    say('boot ok, desktop class ' + document.documentElement.classList.contains('desktop') + ', page scrolls ' + (document.scrollingElement.scrollHeight > innerHeight));
    let env = null;
    for (let i = 0; i < 50 && !env?.calibration; i++, await new Promise((r) => setTimeout(r, 100))) {
      env = await T.invoke('api', { method: 'GET', path: '/api/env', body: null });
    }
    say('timing floor at startup ' + env?.calibration?.floor_p50_us?.toFixed(1) + ' µs');
    const boot = await T.invoke('api', { method: 'GET', path: '/api/bootstrap', body: null });
    const cfg = { ...boot.defaults, resolvers: ['cloudflare', 'google'], samples: 5, probes: false, cdn: false };
    const { id } = await T.invoke('api', { method: 'POST', path: '/api/runs', body: cfg });
    history.pushState(null, '', '/run/' + id);
    dispatchEvent(new PopStateEvent('popstate'));
    say('live view ' + !!(await until(() => text().includes('Measuring'), 5000)));
    const done = await until(() => document.querySelector('.toolbar h1')?.textContent?.trim() === 'Results', 90000);
    if (!done) return say('FAIL run never finished in the UI: ' + text().slice(0, 300));
    say('run ' + id + ': ' + (document.querySelector('.toolbar .title p')?.textContent ?? '').replace(/\s+/g, ' ').trim());
    say('opens on ' + document.querySelector('[role=tab][aria-selected=true]')?.textContent?.trim());
    [...document.querySelectorAll('[role=tab]')].find((t) => t.textContent.trim() === 'Capabilities')?.click();
    const th = await until(() => document.querySelector('tbody th[scope=row][title]'), 3000);
    if (!th) return say('FAIL no titled row header in Capabilities');
    // The real cursor may be over the window, and WebKit hovers whatever lands under it as the table lays out, which
    // moves the tip off this header. Only the synthetic hover counts here.
    const real = (e) => e.isTrusted && e.stopImmediatePropagation();
    for (const k of ['pointerover', 'pointerout', 'pointermove']) addEventListener(k, real, true);
    const at = th.getBoundingClientRect();
    th.dispatchEvent(new PointerEvent('pointerover', { bubbles: true, pointerType: 'mouse', clientX: at.x + 4, clientY: at.y + 4 }));
    const tip = await until(() => document.querySelector('.tt:popover-open'), 2000);
    th.dispatchEvent(new PointerEvent('pointerout', { bubbles: true, pointerType: 'mouse' }));
    for (const k of ['pointerover', 'pointerout', 'pointermove']) removeEventListener(k, real, true);
    if (!tip) return say('FAIL tooltip never showed');
    say('tooltip ' + JSON.stringify(tip.textContent.slice(0, 40)) + ', title back after leaving ' + th.hasAttribute('title'));
    const err = await T.invoke('api', { method: 'GET', path: '/api/nope', body: null }).then(() => null, (e) => e);
    say('error shape ' + JSON.stringify(err));
    const sub = await T.invoke('subscribe', { runId: 'nope', onEvent: '__CHANNEL__:' + T.transformCallback(() => {}) }).then(() => null, (e) => e);
    say('subscribe to a run that is not live ' + JSON.stringify(sub));
    const exp = await T.invoke('export_run', { id: 'nope', format: 'csv' }).then(() => null, (e) => e);
    say('export a missing run ' + JSON.stringify(exp));
    env = await T.invoke('api', { method: 'POST', path: '/api/calibrate', body: {} });
    say('timing floor after a run, window open ' + env.calibration.floor_p50_us.toFixed(1) + ' µs');
    say('menu?');
    const moved = await until(() => location.pathname === '/reliability' && text().includes('Reliability'), 5000);
    say(moved ? 'PASS' : 'FAIL menu navigation, at ' + location.pathname);
  } catch (e) {
    say('FAIL ' + (e?.error ?? e?.message ?? e));
  }
})();"#;
