/* rupu.sh — the mark. A Canvas2D light renderer for the introduction and the
   hero: the ∞ gathers from particles, a comet ignites it, and it lives as a
   lit tube with strands of energy braided around it, pulses racing along the
   strands and diffusing into the body. The camera flies it into the canvas
   as the site fades in; it untwists into the loop of edges, thins into the
   real graph, and folds back when the run ends.

   Geometry comes from the DOM: the brand mark (favicon path) and the stage's
   own edge paths. The scene in site.js drives the run and calls back here. */
(function () {
  "use strict";
  var reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  var canvasEl = document.querySelector(".rg-canvas"), stage = canvasEl && canvasEl.querySelector(".rg-stage");
  if (!canvasEl || !stage || reduced || !window.rupuScene) return;
  var W = Number(canvasEl.getAttribute("data-w")), H = Number(canvasEl.getAttribute("data-h"));
  var scene = window.rupuScene;

  /* ── geometry, in stage coordinates ───────────────────────── */
  var N = 240, markPts, loopPts, peanut, markK, markCx = W / 2, markCy = H / 2;
  var SVG = "http://www.w3.org/2000/svg";
  function samplePath(el, n, a, b) {
    var L = el.getTotalLength(), out = [];
    for (var i = 0; i < n; i++) { var t = a + (b - a) * i / (n - 1); var q = el.getPointAtLength(t * L); out.push([q.x, q.y]); }
    return out;
  }
  function sampleLine(a, b, n) { var out = []; for (var i = 0; i < n; i++) { var t = i / (n - 1); out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]); } return out; }
  function resample(pts, n) {
    var segs = [], total = 0;
    for (var i = 1; i < pts.length; i++) { var d = Math.hypot(pts[i][0] - pts[i - 1][0], pts[i][1] - pts[i - 1][1]); segs.push(d); total += d; }
    var rs = [], acc = 0, j = 0;
    for (var k = 0; k < n; k++) {
      var want = total * k / (n - 1);
      while (j < segs.length - 1 && acc + segs[j] < want) { acc += segs[j]; j++; }
      var t = segs[j] ? (want - acc) / segs[j] : 0;
      rs.push([pts[j][0] + (pts[j + 1][0] - pts[j][0]) * t, pts[j][1] + (pts[j + 1][1] - pts[j][1]) * t]);
    }
    return rs;
  }
  function flipLeft(pts, theta) {
    var c = Math.cos(theta), out = new Array(pts.length);
    for (var i = 0; i < pts.length; i++) { var x = pts[i][0], y = pts[i][1]; out[i] = x < markCx ? [x, markCy + (y - markCy) * c] : [x, y]; }
    return out;
  }
  function build() {
    var hidden = document.createElementNS(SVG, "svg"); hidden.setAttribute("width", "0"); hidden.setAttribute("height", "0"); hidden.style.position = "absolute";
    var raw = document.createElementNS(SVG, "path");
    raw.setAttribute("d", "M21 41c-5 0-9-4-9-9s4-9 9-9c4 0 7 3 11 9 4 6 7 9 11 9 5 0 9-4 9-9s-4-9-9-9c-4 0-7 3-11 9-4 6-7 9-11 9z");
    hidden.appendChild(raw); document.body.appendChild(hidden);
    markK = Math.min(W / 42, H / 22) * 0.86;
    markPts = samplePath(raw, N, 0, 1).map(function (q) { return [(q[0] - 32) * markK + markCx, (q[1] - 32) * markK + markCy]; });
    document.body.removeChild(hidden);
    var edge = function (id) { return stage.querySelector('.gx-edge[data-edge="' + id + '"] .gx-e-base'); };
    var e2 = edge("e2"), e4 = edge("e4"), e5 = edge("e5"), e3 = edge("e3");
    if (!e2 || !e4 || !e5 || !e3) return false;
    var p2 = samplePath(e2, 30, 0.22, 1), p4 = samplePath(e4, 30, 0, 0.78), p5 = samplePath(e5, 30, 0.78, 0), p3 = samplePath(e3, 30, 1, 0.22);
    var top = sampleLine(p2[p2.length - 1], p4[0], 50), bottom = sampleLine(p5[p5.length - 1], p3[0], 50);
    var right = sampleLine(p4[p4.length - 1], p5[0], 10), left = sampleLine(p3[p3.length - 1], p2[0], 10);
    loopPts = resample(p2.concat(top, p4, right, p5, bottom, p3, left), N);
    peanut = flipLeft(markPts, Math.PI);
    var best = null;
    [false, true].forEach(function (rev) {
      var m = rev ? peanut.slice().reverse() : peanut;
      for (var off = 0; off < N; off += 2) {
        var cost = 0;
        for (var q = 0; q < N; q += 4) { var a = m[(q + off) % N], b = loopPts[q]; cost += Math.hypot(a[0] - b[0], a[1] - b[1]); }
        if (!best || cost < best.cost) best = { cost: cost, rev: rev, off: off };
      }
    });
    if (best.rev) markPts = markPts.slice().reverse();
    markPts = markPts.slice(best.off).concat(markPts.slice(0, best.off));
    peanut = flipLeft(markPts, Math.PI);
    return true;
  }
  if (!build()) return;
  window.rupuMarkReady = true;

  var easeInOut = function (t) { return t < .5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2; };
  function lerpPts(a, b, e) { var o = new Array(N); for (var i = 0; i < N; i++) o[i] = [a[i][0] + (b[i][0] - a[i][0]) * e, a[i][1] + (b[i][1] - a[i][1]) * e]; return o; }
  var FLIP = 0.48;
  function unwindShape(t) {
    if (t < FLIP) return flipLeft(markPts, easeInOut(t / FLIP) * Math.PI);
    return lerpPts(peanut, loopPts, easeInOut((t - FLIP) / (1 - FLIP)));
  }

  /* ── the canvas ───────────────────────────────────────────── */
  var cv = document.createElement("canvas"); cv.className = "mark-canvas"; document.body.appendChild(cv);
  var ctx = cv.getContext("2d");
  var glow = document.createElement("canvas"), gctx = glow.getContext("2d");
  var DPR = Math.min(1.5, window.devicePixelRatio || 1), GS = 0.35, vw = 0, vh = 0;
  function resize() {
    vw = window.innerWidth; vh = window.innerHeight;
    cv.width = Math.round(vw * DPR); cv.height = Math.round(vh * DPR); cv.style.width = vw + "px"; cv.style.height = vh + "px";
    glow.width = Math.round(vw * DPR * GS); glow.height = Math.round(vh * DPR * GS);
  }
  resize(); window.addEventListener("resize", resize);

  /* a soft light sprite, drawn scaled: the diffusion of every pulse and particle */
  var sprite = document.createElement("canvas"); sprite.width = sprite.height = 64;
  (function () { var c = sprite.getContext("2d"), g = c.createRadialGradient(32, 32, 0, 32, 32, 32); g.addColorStop(0, "rgba(255,255,255,1)"); g.addColorStop(0.18, "rgba(255,255,255,0.75)"); g.addColorStop(0.45, "rgba(255,255,255,0.22)"); g.addColorStop(1, "rgba(255,255,255,0)"); c.fillStyle = g; c.fillRect(0, 0, 64, 64); })();
  var tint = {};
  function tinted(rgb) {
    var key = rgb.join(",");
    if (tint[key]) return tint[key];
    var c = document.createElement("canvas"); c.width = c.height = 64; var x = c.getContext("2d");
    x.drawImage(sprite, 0, 0); x.globalCompositeOperation = "source-in"; x.fillStyle = "rgb(" + key + ")"; x.fillRect(0, 0, 64, 64);
    x.globalCompositeOperation = "lighter"; x.globalAlpha = 0.6; x.drawImage(sprite, 16, 16, 32, 32);
    return (tint[key] = c);
  }
  function blob(c, img, x, y, r, a) { if (a <= 0.003 || r <= 0.2) return; c.globalAlpha = a; c.drawImage(img, x - r, y - r, r * 2, r * 2); }

  /* the view maps stage space to the screen: screen = (p - c) * s + o */
  function stageView() {
    var r = canvasEl.getBoundingClientRect();
    var s = parseFloat(getComputedStyle(canvasEl).getPropertyValue("--s")) || 1;
    var pad = parseFloat(getComputedStyle(canvasEl).getPropertyValue("--pad")) || 28;
    return { x: r.left + pad + markCx * s, y: r.top + pad + markCy * s, s: s };
  }
  function introView() { var size = Math.min(vw * 0.74, 820), s = size / (markK * 40); return { x: vw / 2, y: vh * 0.5, s: s }; }
  var view = introView();
  function toScreen(pts) { var o = new Array(pts.length); for (var i = 0; i < pts.length; i++) o[i] = [(pts[i][0] - markCx) * view.s + view.x, (pts[i][1] - markCy) * view.s + view.y]; return o; }

  /* ── one material: light particles orbiting the path ───────── */
  var PAL = [[124, 58, 237], [146, 101, 246], [167, 139, 250], [196, 181, 253], [237, 233, 254]];
  function colAt(u, a) { var k = ((u % 1) + 1) % 1 * (PAL.length - 1), i = Math.floor(k), f = k - i, A = PAL[i], B = PAL[Math.min(i + 1, PAL.length - 1)]; return "rgba(" + Math.round(A[0] + (B[0] - A[0]) * f) + "," + Math.round(A[1] + (B[1] - A[1]) * f) + "," + Math.round(A[2] + (B[2] - A[2]) * f) + "," + a + ")"; }
  var TWIST = 2.0, OMEGA = 0.00045, P = 1500, parts = [];
  for (var i = 0; i < P; i++) {
    var tier = Math.random();
    parts.push({ p: Math.random(), ph: Math.random() * 6.283, v: (0.000025 + Math.random() * 0.00006) * (tier < 0.15 ? 2.2 : 1),
      size: tier < 0.08 ? 1.6 + Math.random() : 0.45 + Math.random() * 0.9, rgb: PAL[Math.min(PAL.length - 1, Math.floor(Math.pow(Math.random(), 1.6) * PAL.length))],
      lane: 0.55 + Math.random() * 0.6, sx: 0, sy: 0, jx: (Math.random() - 0.5), jy: (Math.random() - 0.5) });
  }
  function seedCloud() {
    var pts = toScreen(markPts), cx = 0, cy = 0;
    for (var i = 0; i < N; i++) { cx += pts[i][0]; cy += pts[i][1]; }
    cx /= N; cy /= N; var R = (markK * 40 * view.s) * 0.95;
    parts.forEach(function (q) { var a = Math.random() * 6.283, r = R * (0.25 + Math.random() * 1.5); q.sx = cx + Math.cos(a) * r; q.sy = cy + Math.sin(a) * r * 0.72; });
  }

  /* ── drawing ──────────────────────────────────────────────── */
  var state = { shape: markPts, w: 52, form: 0, alpha: 1, energy: 0 };
  var W_MARK = 52, W_EDGE = 2.5;
  function normals(pts) {
    var o = new Array(pts.length);
    for (var i = 0; i < pts.length; i++) { var a = pts[(i + 1) % pts.length], b = pts[(i - 1 + pts.length) % pts.length]; var dx = a[0] - b[0], dy = a[1] - b[1], L = Math.hypot(dx, dy) || 1; o[i] = [-dy / L, dx / L]; }
    return o;
  }
  function wave(u, now) { /* brightness travelling along the path: three pulses, two directions */
    return 0.55 + 0.25 * Math.pow(0.5 + 0.5 * Math.sin((u * 3 - now / 2600) * 6.283), 3) + 0.2 * Math.pow(0.5 + 0.5 * Math.sin((u * 2 + now / 4100) * 6.283 + 1.7), 4);
  }
  function corePath(c, pts, R, now, alpha) {
    /* the faint spine that anchors the ring, in short runs of flowing hue */
    var n = pts.length, step = 8; c.lineCap = "round"; c.lineJoin = "round"; c.lineWidth = Math.max(1, R * 0.5);
    for (var i = 0; i < n; i += step) {
      var j = Math.min(n, i + step); c.strokeStyle = colAt(i / n * 1.3 - now / 9000, 0.28 * alpha);
      c.beginPath(); c.moveTo(pts[i][0], pts[i][1]); for (var k = i + 1; k <= j; k++) c.lineTo(pts[k % n][0], pts[k % n][1]); c.stroke();
    }
  }
  function particlePass(c, pts, nrm, R, now, dt, scaleR, alphaMul, every) {
    var n = pts.length, f = state.form, E = state.energy, A = state.alpha;
    c.globalCompositeOperation = "lighter";
    for (var i = 0; i < parts.length; i += every) {
      var q = parts[i];
      if (every === 1) q.p = ((q.p + q.v * dt * (0.35 + 0.65 * E) * (1 + 0.8 * (wave(q.p, now) - 0.55))) % 1 + 1) % 1;
      var idx = Math.floor(q.p * n) % n, u = q.p;
      var phi = q.ph + u * 6.283 * TWIST + now * OMEGA, dep = Math.sin(phi), lane = Math.cos(phi) * q.lane;
      var tx = pts[idx][0] + nrm[idx][0] * lane * R, ty = pts[idx][1] + nrm[idx][1] * lane * R;
      var x = q.sx + (tx - q.sx) * f, y = q.sy + (ty - q.sy) * f;
      var wv = wave(u, now), front = (dep + 1) / 2;
      var r = Math.max(0.7, R * 0.14 * q.size) * (0.55 + 0.45 * front) * (0.8 + 0.5 * E * wv) * scaleR;
      var a = (0.25 + 0.75 * f) * (0.3 + 0.7 * front) * (0.35 + 0.65 * E * wv + 0.2 * (1 - E)) * A * alphaMul * 0.42;
      blob(c, tinted(q.rgb), x, y, r * 2.4, Math.min(0.9, a));
    }
    c.globalAlpha = 1; c.globalCompositeOperation = "source-over";
  }

  var last = 0, raf = 0;
  function frame(now) {
    var dt = last ? Math.min(40, now - last) : 16; last = now;
    var pts = toScreen(state.shape), nrm = normals(pts), R = Math.max(2.2, state.w * view.s / 2), A = state.alpha, E = state.energy;
    ctx.setTransform(DPR, 0, 0, DPR, 0, 0);
    ctx.globalCompositeOperation = "destination-out"; ctx.fillStyle = "rgba(0,0,0,0.55)"; ctx.fillRect(0, 0, vw, vh);
    ctx.globalCompositeOperation = "source-over"; ctx.globalAlpha = 1;
    if (A <= 0.003) { raf = requestAnimationFrame(frame); return; }
    /* the field: a few of the particles drawn large and soft into a small canvas, blurred back up */
    gctx.setTransform(DPR * GS, 0, 0, DPR * GS, 0, 0); gctx.clearRect(0, 0, vw, vh);
    particlePass(gctx, pts, nrm, R, now, dt, 3.2, 0.9 * (0.3 + 0.7 * E) * state.form, 5);
    ctx.save(); ctx.globalCompositeOperation = "lighter";
    ctx.filter = "blur(" + Math.round(14 + R * 0.6) + "px)"; ctx.globalAlpha = 0.7 * A;
    ctx.drawImage(glow, 0, 0, glow.width, glow.height, 0, 0, vw, vh);
    ctx.filter = "blur(" + Math.round(4 + R * 0.2) + "px)"; ctx.globalAlpha = 0.35 * A;
    ctx.drawImage(glow, 0, 0, glow.width, glow.height, 0, 0, vw, vh);
    ctx.restore(); ctx.filter = "none"; ctx.globalAlpha = 1;
    corePath(ctx, pts, R, now, A * E * state.form);
    particlePass(ctx, pts, nrm, R, now, dt, 1, 1, 1);
    raf = requestAnimationFrame(frame);
  }

  /* ── choreography ─────────────────────────────────────────── */
  function tween(ms, ease, fn, then) {
    var t0 = 0; (function step(now) { if (!t0) t0 = now; var t = Math.min(1, (now - t0) / ms); fn(ease ? ease(t) : t, t); if (t < 1) requestAnimationFrame(step); else if (then) then(); })(performance.now());
  }
  var intro = document.querySelector(".intro"), introOn = document.documentElement.classList.contains("intro-on") && intro;
  var T = { form: 2200, energise: 1500, hold: 1000, fly: 2200, settle: 600, unwind: 3200, fold: 2600 };
  function fadeCanvas(to, ms, then) { var from = state.alpha; tween(ms, null, function (e) { state.alpha = from + (to - from) * e; }, then); }

  function gather(done, scale) {
    /* the dots gather onto the path; then the ring comes alive: brighter, faster, breathing */
    state.shape = markPts; state.w = W_MARK; state.form = 0; state.alpha = 1; state.energy = 0;
    seedCloud();
    tween(T.form * scale, easeInOut, function (e) { state.form = e; }, function () {
      tween(T.energise * scale, easeInOut, function (e) { state.energy = e; }, function () { setTimeout(done, T.hold); });
    });
  }
  function playIntro(done) {
    stage.setAttribute("data-phase", "mark");
    gather(function () {
      var from = { x: view.x, y: view.y, s: view.s }, to = stageView();
      document.documentElement.classList.add("intro-reveal");
      intro.setAttribute("data-lift", "");
      tween(T.fly, easeInOut, function (e) { view.x = from.x + (to.x - from.x) * e; view.y = from.y + (to.y - from.y) * e; view.s = from.s + (to.s - from.s) * e; }, function () {
        view = stageView();
        document.documentElement.classList.add("intro-done");
        intro.remove();
        try { sessionStorage.setItem("rupu.intro", "1"); } catch (e) {}
        setTimeout(done, T.settle);
      });
    }, 1);
  }
  function playMarkOnly(done) { stage.setAttribute("data-phase", "mark"); view = stageView(); gather(done, 0.8); }
  function unwind() {
    stage.setAttribute("data-phase", "morph");
    var landed = false;
    tween(T.unwind, null, function (t) {
      state.shape = unwindShape(t); state.w = W_MARK + (W_EDGE - W_MARK) * easeInOut(t);
      if (!landed && t >= 0.8) { landed = true; scene.startRun(); }
    }, function () { stage.setAttribute("data-phase", "run"); fadeCanvas(0, 900); });
  }
  function fold() {
    view = stageView();
    state.shape = loopPts; state.w = W_EDGE; state.form = 1; state.energy = 1;
    stage.setAttribute("data-phase", "morph");
    fadeCanvas(1, 600);
    tween(T.fold, null, function (t) { state.shape = unwindShape(1 - t); state.w = W_EDGE + (W_MARK - W_EDGE) * easeInOut(t); }, function () { setTimeout(unwind, T.hold); });
  }
  scene.onLoopEnd = fold;

  var started = false;
  var io = new IntersectionObserver(function (entries) {
    entries.forEach(function (en) {
      if (en.isIntersecting) {
        if (!raf) { last = 0; raf = requestAnimationFrame(frame); }
        if (!started) { started = true; if (introOn) playIntro(unwind); else playMarkOnly(unwind); }
      } else if (raf && started && stage.getAttribute("data-phase") === "run") { cancelAnimationFrame(raf); raf = 0; }
    });
  }, { threshold: 0.1 });
  io.observe(canvasEl);
  raf = requestAnimationFrame(frame);
})();
