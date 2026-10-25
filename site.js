/* rupu.sh — the hero's run-graph scene and the copy buttons, nothing else.
   The scene plays a workflow the way the control plane shows one: cards
   enter column by column, edges draw in, work flows along live edges, the
   gate waits on the operator, each step blooms as it lands. Then it resets
   and plays again. The markup and stylesheet are the CP's own (graph.css). */
(function () {
  "use strict";

  var reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  /* ── copy buttons ───────────────────────────────────────── */
  document.querySelectorAll(".copy[data-copy]").forEach(function (btn) {
    btn.addEventListener("click", function () {
      var text = btn.getAttribute("data-copy");
      var done = function () {
        btn.textContent = "Copied";
        btn.setAttribute("data-done", "");
        setTimeout(function () { btn.textContent = "Copy"; btn.removeAttribute("data-done"); }, 1600);
      };
      if (navigator.clipboard && navigator.clipboard.writeText) navigator.clipboard.writeText(text).then(done, done);
      else done();
    });
  });

  /* ── showcase clips: play in view, pause out of it ───────── */
  (function () {
    var clips = Array.prototype.slice.call(document.querySelectorAll("video.clip"));
    if (!clips.length) return;
    clips.forEach(function (v) {
      var body = v.closest(".clip-wrap"), btn = body.querySelector(".clip-play");
      if (reduced) {
        body.setAttribute("data-paused", "");
        btn.addEventListener("click", function () { body.removeAttribute("data-paused"); v.play(); });
        return;
      }
      v.addEventListener("error", function () { body.setAttribute("data-paused", ""); }, true);
    });
    if (reduced || !("IntersectionObserver" in window)) return;
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (en) {
        var v = en.target;
        if (en.isIntersecting) { if (v.preload === "none") v.preload = "auto"; var p = v.play(); if (p && p.catch) p.catch(function () {}); }
        else v.pause();
      });
    }, { threshold: 0.35 });
    clips.forEach(function (v) { io.observe(v); });
  })();

  /* ── section entrances: heading, text, then the picture ──── */
  (function () {
    var secs = Array.prototype.slice.call(document.querySelectorAll("main > section:not(.hero), footer"));
    if (reduced || !("IntersectionObserver" in window)) return;
    secs.forEach(function (s) { s.classList.add("reveal"); });
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (en) { if (en.isIntersecting) { en.target.classList.add("in"); io.unobserve(en.target); } });
    }, { threshold: 0.18, rootMargin: "0px 0px -8% 0px" });
    secs.forEach(function (s) { io.observe(s); });
  })();

  /* ── latest release: the stable channel ─────────────────── */
  (function () {
    var verEl = document.getElementById("dl-ver"), btn = document.getElementById("dl-btn");
    if (!verEl || !btn) return;
    fetch("https://api.github.com/repos/Section9Labs/rupu/releases/latest", { headers: { Accept: "application/vnd.github+json" } })
      .then(function (r) { return r.ok ? r.json() : null; })
      .then(function (rel) {
        if (!rel) return;
        var ver = (rel.tag_name || "").replace(/-(cli|build|beta)$/, "");
        var assets = rel.assets || [];
        var ua = (navigator.userAgent || "") + " " + (navigator.platform || "");
        var isMac = /Mac|iPhone|iPad/i.test(ua);
        var isArm = /arm|aarch64/i.test(ua) || (isMac && navigator.maxTouchPoints > 1);
        var want, label;
        if (isMac) { want = "rupu-darwin-arm64"; label = "macOS · Apple Silicon"; }
        else if (/Linux|X11|Android/i.test(ua)) { want = isArm ? "rupu-linux-arm64" : "rupu-linux-x64"; label = "Linux · " + (isArm ? "arm64" : "x86_64"); }
        else { want = "rupu-darwin-arm64"; label = "macOS · Apple Silicon"; }
        var byName = function (n) { return assets.filter(function (a) { return a.name === n; })[0]; };
        var bin = byName(want) || byName("rupu-darwin-arm64");
        if (!bin) return;
        btn.href = bin.browser_download_url; btn.removeAttribute("target");
        verEl.textContent = (ver ? ver + " " : "") + label;
      }).catch(function () {});
  })();

  /* ── hero scene: the run the graph plays ─────────────────── */
  var canvas = document.querySelector(".rg-canvas");
  if (!canvas) return;
  var stage = canvas.querySelector(".rg-stage");
  var W = Number(canvas.getAttribute("data-w")), H = Number(canvas.getAttribute("data-h"));
  canvas.style.setProperty("--gh", H);
  function fit() {
    var pad = parseFloat(getComputedStyle(canvas).getPropertyValue("--pad")) || 28;
    canvas.style.setProperty("--s", Math.min(1, (canvas.clientWidth - pad * 2) / W).toFixed(4));
  }
  fit(); window.addEventListener("resize", fit);

  var WORD = { running: "running", done: "done", failed: "failed", awaiting_approval: "needs you", paused: "paused", pending: "pending", skipped: "skipped" };
  var LOUD = { running: 1, failed: 1, awaiting_approval: 1, paused: 1 };
  function paintCapsule(cap, state) {
    if (!cap) return;
    cap.setAttribute("data-state", state);
    var g = cap.querySelector(".gx-glyph"); if (g) g.setAttribute("data-state", state);
    var w = cap.querySelector(".gx-word");
    if (w) { w.textContent = WORD[state]; w.className = LOUD[state] ? "gx-word" : "gx-word sr-only"; }
    cap.classList.toggle("gx-cap-quiet", !LOUD[state]);
    cap.title = LOUD[state] ? "" : WORD[state];
  }
  var nodes = {}; stage.querySelectorAll(".gx-node").forEach(function (n) { nodes[n.getAttribute("data-node")] = n; });
  var subs = {}; stage.querySelectorAll("[data-sub]").forEach(function (el) { subs[el.getAttribute("data-sub")] = el; });
  var edges = Array.prototype.slice.call(stage.querySelectorAll(".gx-edge"));
  var summary = canvas.querySelector(".gx-summary");
  var counter = document.querySelector("[data-count]");
  function cue(el, kind) { el.removeAttribute("data-cue"); void el.offsetWidth; el.setAttribute("data-cue", kind); setTimeout(function () { if (el.getAttribute("data-cue") === kind) el.removeAttribute("data-cue"); }, 1100); }
  function setState(id, state) {
    var n = nodes[id]; if (!n) return;
    var was = n.getAttribute("data-state");
    n.setAttribute("data-state", state);
    paintCapsule(n.querySelector(".gx-head .gx-cap"), state);
    if (state === "done" && was !== "done") cue(n, "done");
    if (state === "failed" && was !== "failed") cue(n, "failed");
    if (n.getAttribute("data-kind") === "gate") {
      if (state === "awaiting_approval") n.setAttribute("data-gate", "wait");
      else if (state === "running") n.setAttribute("data-gate", "approved");
      else n.removeAttribute("data-gate");
    }
  }
  function setSub(key, state) {
    var el = subs[key]; if (!el) return;
    var was = el.getAttribute("data-state");
    el.setAttribute("data-state", state);
    var g = el.querySelector(":scope > .gx-glyph"); if (g) g.setAttribute("data-state", state);
    paintCapsule(el.querySelector(".gx-cap"), state);
    if (state === "done" && was !== "done") cue(el, "done");
    var nid = key.split(":")[0], count = stage.querySelector('[data-count-of="' + nid + '"]');
    if (count) {
      var all = Object.keys(subs).filter(function (k) { return k.indexOf(nid + ":") === 0; });
      count.textContent = all.filter(function (k) { return subs[k].getAttribute("data-state") === "done"; }).length + "/" + all.length;
    }
  }
  function paintEdges() {
    edges.forEach(function (e) {
      var to = nodes[e.getAttribute("data-to")], s = to ? to.getAttribute("data-state") : "pending";
      e.setAttribute("data-tone", s === "running" ? "live" : s === "awaiting_approval" ? "await" : s === "done" ? "lit" : "idle");
    });
    if (summary) {
      var c = { done: 0, running: 0, awaiting: 0 };
      Object.keys(nodes).forEach(function (id) { var s = nodes[id].getAttribute("data-state"); if (s === "done") c.done++; else if (s === "running") c.running++; else if (s === "awaiting_approval") c.awaiting++; });
      summary.querySelector('[data-sum="done"]').textContent = c.done;
      summary.querySelector('[data-sum="running"]').textContent = c.running;
      summary.querySelector('[data-sum="awaiting"]').textContent = c.awaiting;
      if (c.awaiting) summary.setAttribute("data-wait", ""); else summary.removeAttribute("data-wait");
    }
  }
  function resetAll() {
    Object.keys(nodes).forEach(function (id) { nodes[id].removeAttribute("data-cue"); setState(id, "pending"); });
    Object.keys(subs).forEach(function (k) { subs[k].removeAttribute("data-cue"); setSub(k, "pending"); });
    var r = stage.querySelector('[data-round-of="n4"]'); if (r) r.textContent = "1";
    paintEdges();
  }
  var script = [
    [0,     function () { setState("n1", "running"); }],
    [1800,  function () { setState("n1", "done"); setState("n2", "running"); }],
    [2300,  function () { setSub("n2:0", "running"); setSub("n2:1", "running"); setSub("n2:2", "running"); setSub("n2:3", "running"); }],
    [3300,  function () { setSub("n2:0", "done"); setSub("n2:4", "running"); }],
    [3700,  function () { setSub("n2:2", "done"); setSub("n2:5", "running"); }],
    [4200,  function () { setSub("n2:1", "done"); setSub("n2:3", "done"); }],
    [4900,  function () { setSub("n2:4", "done"); setSub("n2:5", "done"); setState("n2", "done");
                          setState("n3", "running"); setSub("n3:0", "running"); setSub("n3:1", "running");
                          setState("n4", "running"); setSub("n4:0", "running"); setSub("n4:1", "running"); }],
    [7200,  function () { setSub("n3:1", "done"); }],
    [7900,  function () { setSub("n4:0", "done"); }],
    [8600,  function () { setSub("n4:1", "done"); var r = stage.querySelector('[data-round-of="n4"]'); if (r) r.textContent = "2"; setSub("n4:0", "running"); setSub("n4:1", "running"); }],
    [9300,  function () { setSub("n3:0", "done"); setState("n3", "done"); }],
    [10600, function () { setSub("n4:0", "done"); setSub("n4:1", "done"); setState("n4", "done"); setState("n5", "awaiting_approval"); }],
    [14200, function () { setState("n5", "running"); }],
    [15600, function () { setState("n5", "done"); }]
  ];
  var LOOP_AT = 18600;
  var tokens = 0, tokenFrom = 0, tokenTarget = 0, tokenT0 = 0;
  function bumpTokens(by) { tokenFrom = tokens; tokenTarget = tokens + by; tokenT0 = performance.now(); }
  function tickTokens(now) { if (!counter) return; var t = Math.min(1, (now - tokenT0) / 750), e = 1 - Math.pow(1 - t, 3); tokens = Math.round(tokenFrom + (tokenTarget - tokenFrom) * e); counter.textContent = tokens.toLocaleString("en-US"); }
  var playStart = 0, cursor = 0, raf = 0, pausedAt = 0, ended = false;
  function play(now) {
    if (!playStart) { playStart = now; cursor = 0; }
    if (pausedAt) { playStart += now - pausedAt; pausedAt = 0; }
    var t = now - playStart;
    while (cursor < script.length && script[cursor][0] <= t) { script[cursor][1](); paintEdges(); bumpTokens(900 + Math.round(Math.random() * 2400)); cursor++; }
    tickTokens(now);
    if (t >= LOOP_AT && !ended) { ended = true; raf = 0; (api.onLoopEnd || restart)(); return; }
    raf = requestAnimationFrame(play);
  }
  function startRun() {
    /* the cards and edges enter where the mark became the loop */
    resetAll();
    stage.setAttribute("data-reset", ""); void stage.offsetWidth; stage.removeAttribute("data-reset");
    stage.setAttribute("data-phase", "landing");
    tokens = tokenFrom = tokenTarget = 0; if (counter) counter.textContent = "0";
    playStart = 0; cursor = 0; ended = false; pausedAt = 0;
    if (raf) cancelAnimationFrame(raf);
    raf = requestAnimationFrame(play);
  }
  function restart() { stage.setAttribute("data-reset", ""); setTimeout(startRun, 420); }
  var api = { startRun: startRun, onLoopEnd: null };
  window.rupuScene = api;
  resetAll();

  if (reduced) {
    setState("n1", "done"); setState("n2", "done");
    ["n2:0","n2:1","n2:2","n2:3","n2:4","n2:5"].forEach(function (k) { setSub(k, "done"); });
    setState("n3", "running"); setSub("n3:0", "done"); setSub("n3:1", "running");
    setState("n4", "done"); setSub("n4:0", "done"); setSub("n4:1", "done");
    paintEdges(); if (counter) counter.textContent = "12,480";
    stage.setAttribute("data-phase", "run");
    return;
  }
  /* pause the run while the hero is off screen; mark.js starts it */
  var io = new IntersectionObserver(function (entries) {
    entries.forEach(function (en) {
      if (en.isIntersecting) { if (!raf && playStart && !ended) raf = requestAnimationFrame(play); }
      else if (raf) { cancelAnimationFrame(raf); raf = 0; pausedAt = performance.now(); }
    });
  }, { threshold: 0.15 });
  io.observe(canvas);
  /* no renderer (old browser): the run plays on its own */
  setTimeout(function () { if (!window.rupuMarkReady && !playStart) { stage.setAttribute("data-phase", "run"); startRun(); } }, 1500);
})();
