// FreEco.ai Starnet: the agent teams' work as a pixel-art station.
//
// Rooms are teams, desks are agents, and each agent moves by what it is doing
// this moment: idle (seated, blinking now and then), thinking (bobbing, with
// a thinking bubble), using a tool (gesturing), waiting on a helper, waiting
// for the owner's yes (a red "!"), or an error (a short shake). The speech
// bubble carries the agent's current task. Click an agent to open its chat.
//
// The look follows StarNet's station view (github.com/FreecoDAO/starnet). The
// renderer is FreEco's own plain canvas code; only StarNet's sprites are used,
// under the MIT License: see static/starnet/THIRD_PARTY_NOTICES.md.
//
// Safety: agent names, task titles and errors reach the screen only through
// canvas fillText or Alpine's x-text, never as HTML. Sprites are fetched from
// /starnet/<name>.png with the dashboard's own auth and decoded with
// createImageBitmap, so the page needs no CSP change and makes no remote load.
'use strict';

(function() {
  // Keep in step with STARNET_FILES in src/webchat.rs (a test checks it).
  var CHARACTERS = ['astronaut', 'robot', 'alien', 'capybara', 'bear', 'skeleton'];
  var POSES = ['sit_south', 'blink_south', 'gesture_south_0', 'gesture_south_2', 'gesture_south_4', 'gesture_south_6'];
  var FURNITURE = ['desk', 'chair', 'plant', 'console', 'screens', 'coffee', 'whiteboard', 'rack'];
  var GESTURE = ['gesture_south_0', 'gesture_south_2', 'gesture_south_4', 'gesture_south_6'];

  // Layout, in unzoomed canvas pixels. A desk sprite is 96x72, a character
  // 92x92 with the figure in the middle, a chair or plant 48x48.
  var CELL_W = 150;
  var CELL_H = 150;
  var ROOM_PAD = 14;
  var WALL_H = 56;
  var ROOM_GAP = 18;
  var POLL_MS = 2000;
  var FRAME_MS = 80;

  var STATE_LABEL = {
    idle: 'idle',
    thinking: 'thinking',
    tool: 'using a tool',
    waiting_on_helper: 'waiting on a helper',
    waiting_for_you: 'waiting for your yes',
    error: 'error'
  };
  var STATE_VAR = {
    idle: '--live-idle',
    thinking: '--live-think',
    tool: '--live-tool',
    waiting_on_helper: '--live-help',
    waiting_for_you: '--live-you',
    error: '--live-fail'
  };

  // Sprites are shared by every visit to the page and fetched once.
  var sprites = {};
  var spritesLoading = null;

  function loadSprite(name) {
    var headers = {};
    var token = (typeof FreecoAPI !== 'undefined' && FreecoAPI.getToken) ? FreecoAPI.getToken() : '';
    if (token) headers['Authorization'] = 'Bearer ' + token;
    return fetch('/starnet/' + name + '.png', { headers: headers, credentials: 'same-origin' })
      .then(function(r) {
        if (!r.ok) throw new Error('HTTP ' + r.status);
        return r.blob();
      })
      .then(function(b) { return createImageBitmap(b); })
      .then(function(img) { sprites[name] = img; })
      .catch(function() { /* that piece is drawn as a plain shape instead */ });
  }

  function loadSprites() {
    if (spritesLoading) return spritesLoading;
    if (typeof createImageBitmap !== 'function' || typeof fetch !== 'function') {
      spritesLoading = Promise.resolve();
      return spritesLoading;
    }
    var names = FURNITURE.map(function(f) { return 'furniture-' + f; });
    CHARACTERS.forEach(function(c) {
      POSES.forEach(function(p) { names.push(c + '-' + p); });
    });
    spritesLoading = Promise.all(names.map(loadSprite)).then(function() {
      // Nothing came back (signed out, daemon restarting): try again next visit.
      if (!Object.keys(sprites).length) spritesLoading = null;
    });
    return spritesLoading;
  }

  function hash(s) {
    var h = 0;
    s = String(s || '');
    for (var i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) | 0;
    return Math.abs(h);
  }

  function span(ms) {
    var s = Math.max(0, Math.round(ms / 1000));
    if (s < 60) return s + 's';
    var m = Math.floor(s / 60);
    if (m < 60) return m + 'm ' + (s % 60 < 10 ? '0' : '') + (s % 60) + 's';
    return Math.floor(m / 60) + 'h ' + (m % 60) + 'm';
  }

  // Same as the Live page: open an agent's chat by its id.
  function openConversation(agentId) {
    var store = Alpine.store('app');
    var agent = (store.agents || []).filter(function(a) { return a.id === agentId; })[0];
    if (!agent) {
      if (typeof FreecoToast !== 'undefined') FreecoToast.error('That agent is not running.');
      return;
    }
    store.pendingSession = null;
    store.pendingAgent = agent;
    location.hash = 'agents';
  }

  // Colours come from theme.css / components.css, so day and night follow
  // the rest of the dashboard.
  function palette(el) {
    var cs = getComputedStyle(el);
    function v(name, fallback) {
      var x = cs.getPropertyValue(name).trim();
      return x || fallback;
    }
    var p = {
      space: v('--sn-space', '#dfe8e4'),
      star: v('--sn-star', 'transparent'),
      floor: v('--sn-floor', '#f4f7f5'),
      grid: v('--sn-grid', 'rgba(0,0,0,0.05)'),
      wall: v('--sn-wall', '#d6e0db'),
      trim: v('--sn-trim', '#0b7a2f'),
      edge: v('--sn-edge', '#b5c0ba'),
      ink: v('--text', '#1d1d1f'),
      dim: v('--text-dim', '#5f6368'),
      bubble: v('--sn-bubble', '#ffffff'),
      bubbleInk: v('--sn-bubble-ink', '#1d1d1f'),
      onAlert: v('--on-swiss', '#ffffff'),
      mono: v('--font-mono', 'monospace'),
      sans: v('--font-sans', 'sans-serif'),
      state: {}
    };
    Object.keys(STATE_VAR).forEach(function(k) { p.state[k] = v(STATE_VAR[k], '#888888'); });
    return p;
  }

  function stars(w, h) {
    var out = [];
    var seed = 7;
    function rnd() { seed = (seed * 16807) % 2147483647; return seed / 2147483647; }
    var n = Math.floor(w * h / 2600);
    for (var i = 0; i < n; i++) out.push({ x: Math.floor(rnd() * w), y: Math.floor(rnd() * h), big: rnd() < 0.12 });
    return out;
  }

  // Rooms flow left to right and wrap; desks fill each room row by row.
  function layout(snap, width) {
    var byId = {};
    (snap.agents || []).forEach(function(a) { byId[a.agent_id] = a; });
    var maxCols = Math.max(1, Math.floor((width - 2 * ROOM_PAD) / CELL_W));
    var rooms = [];
    var desks = [];
    var x = 0;
    var y = 0;
    var rowH = 0;
    var right = 0;
    (snap.rooms || []).forEach(function(r) {
      var members = (r.agents || []).map(function(id) { return byId[id]; }).filter(Boolean);
      // Two desks wide at least, so an empty room still has space for its name.
      var cols = Math.min(maxCols, Math.max(2, members.length));
      var rowsN = Math.max(1, Math.ceil(members.length / cols));
      var w = cols * CELL_W + 2 * ROOM_PAD;
      var h = WALL_H + rowsN * CELL_H + ROOM_PAD;
      if (x > 0 && x + w > width) { x = 0; y += rowH + ROOM_GAP; rowH = 0; }
      var room = {
        id: r.id,
        name: r.name || 'Team',
        project: r.project || '',
        x: x, y: y, w: w, h: h,
        empty: !members.length,
        // A plant in the corner when the last row has a free seat.
        plant: members.length % cols !== 0 || !members.length
      };
      rooms.push(room);
      members.forEach(function(a, i) {
        desks.push({
          agent: a,
          room: room,
          x: x + ROOM_PAD + (i % cols) * CELL_W,
          y: y + WALL_H + Math.floor(i / cols) * CELL_H
        });
      });
      right = Math.max(right, x + w);
      x += w + ROOM_GAP;
      rowH = Math.max(rowH, h);
    });
    return { rooms: rooms, desks: desks, width: right, height: y + rowH };
  }

  // Shorten text with an ellipsis until it fits.
  function fit(ctx, text, maxW) {
    text = String(text || '');
    if (ctx.measureText(text).width <= maxW) return text;
    var lo = 0;
    var hi = text.length;
    while (lo < hi) {
      var mid = Math.ceil((lo + hi) / 2);
      if (ctx.measureText(text.slice(0, mid) + '…').width <= maxW) lo = mid; else hi = mid - 1;
    }
    return text.slice(0, lo) + '…';
  }

  function sprite(name) { return sprites[name] || null; }

  function drawRoom(ctx, p, r) {
    ctx.fillStyle = p.floor;
    ctx.fillRect(r.x, r.y, r.w, r.h);
    ctx.fillStyle = p.grid;
    for (var gx = r.x + 24; gx < r.x + r.w; gx += 24) ctx.fillRect(gx, r.y + WALL_H, 1, r.h - WALL_H);
    for (var gy = r.y + WALL_H + 24; gy < r.y + r.h; gy += 24) ctx.fillRect(r.x, gy, r.w, 1);
    ctx.fillStyle = p.wall;
    ctx.fillRect(r.x, r.y, r.w, WALL_H);
    ctx.fillStyle = p.trim;
    ctx.fillRect(r.x, r.y + WALL_H - 3, r.w, 3);

    var nameW = r.w - 24;
    var board = sprite('furniture-whiteboard');
    if (board && r.w >= 440) {
      ctx.drawImage(board, r.x + r.w - 192 - 10, r.y);
      nameW = r.w - 192 - 34;
    }
    ctx.textAlign = 'left';
    ctx.textBaseline = 'alphabetic';
    ctx.fillStyle = p.ink;
    ctx.font = '700 13px ' + p.sans;
    ctx.fillText(fit(ctx, r.name, nameW), r.x + 12, r.y + 22);
    if (r.project) {
      ctx.fillStyle = p.dim;
      ctx.font = '11px ' + p.mono;
      ctx.fillText(fit(ctx, r.project, nameW), r.x + 12, r.y + 39);
    }

    var plant = sprite('furniture-plant');
    if (r.plant && plant) ctx.drawImage(plant, r.x + r.w - 48 - 8, r.y + r.h - 48 - 8);
    if (r.empty) {
      ctx.fillStyle = p.dim;
      ctx.font = '12px ' + p.sans;
      ctx.fillText('No agents on this team yet', r.x + 14, r.y + WALL_H + 40);
    }

    ctx.strokeStyle = p.edge;
    ctx.lineWidth = 2;
    ctx.strokeRect(r.x + 1, r.y + 1, r.w - 2, r.h - 2);
  }

  // A stand-in figure for when a sprite could not be loaded.
  function drawFigure(ctx, cx, top, colour) {
    ctx.fillStyle = colour;
    ctx.fillRect(cx - 6, top + 26, 12, 12);   // head
    ctx.fillRect(cx - 9, top + 40, 18, 22);   // body
  }

  // What the agent is doing, in a few words, for the line under its name.
  function stateLine(a, nowMs) {
    var s = a.state || 'idle';
    var label = s === 'tool' && a.tool ? 'using ' + a.tool : (STATE_LABEL[s] || s);
    if (s !== 'idle' && s !== 'error' && a.since_ms) label += ' · ' + span(nowMs - a.since_ms);
    return label;
  }

  // The bubble's text, or '' for none.
  function bubbleText(a, t) {
    var s = a.state || 'idle';
    if (s === 'waiting_for_you') return 'Needs your yes' + (a.tool ? ': ' + a.tool : '');
    if (s === 'error') return a.error || 'Something failed';
    if (a.task) return a.task;
    if (s === 'tool') return a.tool || 'using a tool';
    if (s === 'waiting_on_helper') return 'waiting on a helper';
    if (s === 'thinking') return ['.', '. .', '. . .'][Math.floor(t / 400) % 3];
    return '';
  }

  function drawDesk(ctx, p, d, t, view) {
    var a = d.agent;
    var s = a.state || 'idle';
    var colour = p.state[s] || p.state.idle;
    var phase = hash(a.agent_id) % 1000;
    var deskX = d.x + 8;
    var deskY = d.y + 46;
    var floorY = deskY + 72;
    var cx = deskX + 98; // the agent sits at the right end of its desk

    if (view.hover === a.agent_id) {
      ctx.strokeStyle = p.trim;
      ctx.lineWidth = 2;
      ctx.strokeRect(d.x + 2, d.y + 2, CELL_W - 4, CELL_H - 4);
    }

    // A glow on the floor in the state's colour, pulsing while it works.
    if (s !== 'idle') {
      var pulse = s === 'error' || s === 'waiting_for_you' ? 0.45 : 0.25 + 0.15 * Math.sin((t + phase) / 300);
      ctx.save();
      ctx.globalAlpha = pulse;
      ctx.fillStyle = colour;
      ctx.beginPath();
      ctx.ellipse(cx, floorY - 2, 26, 7, 0, 0, Math.PI * 2);
      ctx.fill();
      ctx.restore();
    }

    var chair = sprite('furniture-chair');
    if (chair) ctx.drawImage(chair, cx - 24, floorY - 48);
    var desk = sprite('furniture-desk');
    if (desk) {
      ctx.drawImage(desk, deskX, deskY);
    } else {
      ctx.fillStyle = p.edge;
      ctx.fillRect(deskX + 4, deskY + 30, 88, 36);
      ctx.fillStyle = p.wall;
      ctx.fillRect(deskX + 30, deskY + 8, 34, 22);
    }

    var character = CHARACTERS[hash(a.agent_id) % CHARACTERS.length];
    var pose = 'sit_south';
    var dx = 0;
    var dy = 0;
    if (s === 'tool') {
      pose = GESTURE[Math.floor((t + phase) / 160) % GESTURE.length];
    } else if (s === 'waiting_for_you') {
      pose = Math.floor((t + phase) / 500) % 2 ? 'gesture_south_2' : 'sit_south';
    } else if (s === 'thinking' || s === 'waiting_on_helper') {
      dy = Math.floor((t + phase) / 400) % 2 ? -1 : 0;
    } else if (s === 'error') {
      if ((t + phase) % 2000 < 400) dx = Math.floor(t / 80) % 2 ? 1 : -1;
    } else if (t && (t + phase * 7) % 4200 < 140) {
      pose = 'blink_south';
    }
    var top = floorY - 84 + dy;
    var img = sprite(character + '-' + pose) || sprite(character + '-sit_south');
    if (img) ctx.drawImage(img, cx - 46 + dx, top);
    else drawFigure(ctx, cx + dx, top, colour);

    // A red marker above the head for the two states the owner should see.
    if (s === 'waiting_for_you' || s === 'error') {
      var bob = s === 'waiting_for_you' && Math.floor(t / 300) % 2 ? -2 : 0;
      ctx.fillStyle = colour;
      ctx.fillRect(cx + 10, top + 8 + bob, 13, 15);
      ctx.fillStyle = p.onAlert;
      ctx.font = '700 12px ' + p.mono;
      ctx.textAlign = 'center';
      ctx.fillText(s === 'error' ? '×' : '!', cx + 16.5, top + 20 + bob);
    }

    // Name with a status light, then what it is doing.
    var midX = d.x + CELL_W / 2;
    ctx.textAlign = 'center';
    ctx.fillStyle = p.ink;
    ctx.font = '600 12px ' + p.sans;
    var name = fit(ctx, a.name || 'agent', CELL_W - 26);
    var nameW = ctx.measureText(name).width;
    ctx.fillText(name, midX + 6, floorY + 16);
    ctx.fillStyle = colour;
    ctx.fillRect(Math.round(midX - nameW / 2 - 6), floorY + 7, 8, 8);
    ctx.fillStyle = p.dim;
    ctx.font = '10px ' + p.mono;
    ctx.fillText(fit(ctx, stateLine(a, view.nowMs), CELL_W - 10), midX, floorY + 30);
  }

  function drawBubble(ctx, p, d, t) {
    var a = d.agent;
    var s = a.state || 'idle';
    var text = bubbleText(a, t);
    if (!text) return;
    var colour = p.state[s] || p.state.idle;
    var cx = d.x + 8 + 98;
    var tipY = d.y + 46 + 72 - 84 + 22; // just above the figure's head
    ctx.font = '11px ' + p.sans;
    var label = fit(ctx, text, 170);
    var w = Math.ceil(ctx.measureText(label).width) + 14;
    var h = 20;
    var bx = Math.round(cx - w / 2);
    // Keep the bubble inside its room.
    bx = Math.max(d.room.x + 4, Math.min(bx, d.room.x + d.room.w - w - 4));
    var by = tipY - 6 - h;

    ctx.save();
    if (s === 'idle') ctx.globalAlpha = 0.7;
    ctx.fillStyle = p.bubble;
    ctx.strokeStyle = colour;
    ctx.lineWidth = 2;
    ctx.beginPath();
    if (ctx.roundRect) ctx.roundRect(bx, by, w, h, 6); else ctx.rect(bx, by, w, h);
    ctx.fill();
    ctx.stroke();
    // The tail, pointing at the speaker.
    var tx = Math.max(bx + 8, Math.min(cx, bx + w - 8));
    ctx.beginPath();
    ctx.moveTo(tx - 5, by + h - 1);
    ctx.lineTo(tx, by + h + 6);
    ctx.lineTo(tx + 5, by + h - 1);
    ctx.closePath();
    ctx.fill();
    ctx.beginPath();
    ctx.moveTo(tx - 5, by + h);
    ctx.lineTo(tx, by + h + 6);
    ctx.lineTo(tx + 5, by + h);
    ctx.stroke();
    ctx.fillStyle = p.bubbleInk;
    ctx.textAlign = 'left';
    ctx.textBaseline = 'middle';
    ctx.fillText(label, bx + 7, by + h / 2 + 1);
    ctx.restore();
    ctx.textBaseline = 'alphabetic';
  }

  function draw(ctx, view, t) {
    var p = view.pal;
    var k = view.dpr * view.zoom;
    ctx.setTransform(k, 0, 0, k, 0, 0);
    ctx.imageSmoothingEnabled = false;
    ctx.clearRect(0, 0, view.w, view.h);
    ctx.fillStyle = p.space;
    ctx.fillRect(0, 0, view.w, view.h);
    ctx.fillStyle = p.star;
    view.stars.forEach(function(s) { ctx.fillRect(s.x, s.y, s.big ? 2 : 1, s.big ? 2 : 1); });
    view.lay.rooms.forEach(function(r) { drawRoom(ctx, p, r); });
    view.lay.desks.forEach(function(d) { drawDesk(ctx, p, d, t, view); });
    // Bubbles last, so a neighbour's desk never covers one.
    view.lay.desks.forEach(function(d) { drawBubble(ctx, p, d, t); });
  }

  document.addEventListener('alpine:init', function() {
    Alpine.data('starnetPage', function() {
      // What the canvas draws lives outside Alpine's reactivity: it is read
      // every frame and none of it is shown through bindings.
      var view = {
        snap: { rooms: [], agents: [] },
        lay: { rooms: [], desks: [], width: 0, height: 0 },
        pal: null,
        zoom: 1,
        dpr: 1,
        w: 0,
        h: 0,
        cssW: 0,
        stars: [],
        hover: null,
        skewMs: 0,
        nowMs: Date.now()
      };
      var timers = { poll: null, raf: 0, last: 0 };
      var observers = [];
      var reduced = !!(window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches);

      return {
        agents: [],
        degraded: [],
        error: '',
        loaded: false,
        paused: false,
        zoom: 1,

        start() {
          var self = this;
          var saved = parseInt(localStorage.getItem('freeco-starnet-zoom') || '', 10);
          this.zoom = saved === 1 || saved === 2 ? saved : (window.innerWidth >= 1200 ? 2 : 1);
          view.zoom = this.zoom;
          var canvas = this.$refs.canvas;
          view.pal = palette(canvas);

          canvas.addEventListener('click', function(e) {
            var d = self.hit(e);
            if (d) openConversation(d.agent.agent_id);
          });
          canvas.addEventListener('mousemove', function(e) {
            var d = self.hit(e);
            var id = d ? d.agent.agent_id : null;
            canvas.style.cursor = d ? 'pointer' : 'default';
            canvas.title = d ? self.describe(d.agent) : '';
            if (id !== view.hover) { view.hover = id; self.paint(); }
          });
          canvas.addEventListener('mouseleave', function() {
            view.hover = null;
            self.paint();
          });

          if (typeof ResizeObserver !== 'undefined') {
            var ro = new ResizeObserver(function() {
              var wrap = self.$refs.wrap;
              if (wrap && wrap.clientWidth !== view.cssW) self.relayout();
            });
            ro.observe(this.$refs.wrap);
            observers.push(ro);
          }
          // Day/night switch: repaint at once rather than at the next poll.
          var mo = new MutationObserver(function() {
            if (self.$refs.canvas) { view.pal = palette(self.$refs.canvas); self.paint(); }
          });
          mo.observe(document.body, { attributes: true, attributeFilter: ['data-theme'] });
          observers.push(mo);

          loadSprites().then(function() { self.paint(); });
          this.poll();
          timers.poll = setInterval(function() {
            if (!self.paused && document.visibilityState === 'visible') self.poll();
          }, POLL_MS);
          if (!reduced) this.loop();
        },

        destroy() {
          clearInterval(timers.poll);
          if (timers.raf) cancelAnimationFrame(timers.raf);
          observers.forEach(function(o) { o.disconnect(); });
          observers = [];
        },

        async poll() {
          try {
            var d = await FreecoAPI.get('/api/starnet/state');
            view.snap = { rooms: (d && d.rooms) || [], agents: (d && d.agents) || [] };
            if (d && d.now_ms) view.skewMs = d.now_ms - Date.now();
            this.agents = view.snap.agents.slice();
            this.degraded = (d && d.degraded) || [];
            this.error = '';
          } catch (e) {
            this.error = 'Starnet updates stopped: ' + ((e && e.message) || 'no connection') + '. Retrying.';
          }
          this.loaded = true;
          if (this.$refs.canvas) view.pal = palette(this.$refs.canvas);
          this.relayout();
        },

        loop() {
          var self = this;
          function frame(t) {
            timers.raf = requestAnimationFrame(frame);
            if (t - timers.last < FRAME_MS) return;
            timers.last = t;
            self.paint();
          }
          timers.raf = requestAnimationFrame(frame);
        },

        relayout() {
          var canvas = this.$refs.canvas;
          var wrap = this.$refs.wrap;
          if (!canvas || !wrap) return;
          view.cssW = wrap.clientWidth;
          var worldW = Math.floor(Math.max(320, view.cssW) / view.zoom);
          view.lay = layout(view.snap, worldW);
          worldW = Math.max(worldW, view.lay.width);
          var worldH = Math.max(view.lay.height, 160);
          view.dpr = window.devicePixelRatio || 1;
          if (worldW !== view.w || worldH !== view.h || canvas.width !== Math.round(worldW * view.zoom * view.dpr)) {
            view.w = worldW;
            view.h = worldH;
            canvas.width = Math.round(worldW * view.zoom * view.dpr);
            canvas.height = Math.round(worldH * view.zoom * view.dpr);
            canvas.style.width = (worldW * view.zoom) + 'px';
            canvas.style.height = (worldH * view.zoom) + 'px';
            view.stars = stars(worldW, worldH);
          }
          this.paint();
        },

        paint() {
          var canvas = this.$refs.canvas;
          if (!canvas || !view.pal || !view.w) return;
          var now = Date.now();
          view.nowMs = now + view.skewMs;
          draw(canvas.getContext('2d'), view, reduced ? 0 : now);
        },

        hit(e) {
          var r = this.$refs.canvas.getBoundingClientRect();
          var x = (e.clientX - r.left) / view.zoom;
          var y = (e.clientY - r.top) / view.zoom;
          return view.lay.desks.filter(function(d) {
            return x >= d.x && x < d.x + CELL_W && y >= d.y && y < d.y + CELL_H;
          })[0] || null;
        },

        setZoom(z) {
          this.zoom = z;
          view.zoom = z;
          try { localStorage.setItem('freeco-starnet-zoom', String(z)); } catch (e) { /* private mode */ }
          this.relayout();
        },

        describe(a) {
          var parts = [a.name || 'agent', stateLine(a, view.nowMs)];
          if (a.error) parts.push(a.error);
          if (a.task) parts.push('task: ' + a.task);
          return parts.join(', ');
        },

        openAgent(id) { openConversation(id); }
      };
    });
  });
})();
