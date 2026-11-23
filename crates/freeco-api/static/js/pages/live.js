// FreEco.ai Live: what every agent is doing this second, and Artifacts.
//
// The idea comes from Starnet's station view: one desk per agent with a light
// and a line saying what it is doing, so a glance answers "who is working on
// what". Here it is a page of cards, a ten-minute timeline, who asked whom and
// every step in order, fed by /api/activity (the agent loop's own hooks).
'use strict';

(function() {
  var WINDOW_MS = 10 * 60 * 1000;
  var HELPER_TOOLS = ['agent_delegate', 'agent_send', 'a2a_send'];

  function initials(name) {
    var parts = String(name || '?').replace(/^freeco[-\s]/i, '').split(/[\s\-_]+/).filter(Boolean);
    return ((parts[0] || '?')[0] + ((parts[1] || '')[0] || (parts[0] || '')[1] || '')).toUpperCase();
  }

  function clock(ms) {
    var d = new Date(ms);
    var p = function(n) { return (n < 10 ? '0' : '') + n; };
    return p(d.getHours()) + ':' + p(d.getMinutes()) + ':' + p(d.getSeconds());
  }

  function span(ms) {
    var s = Math.max(0, Math.round(ms / 1000));
    if (s < 60) return s + 's';
    var m = Math.floor(s / 60);
    if (m < 60) return m + 'm ' + (s % 60 < 10 ? '0' : '') + (s % 60) + 's';
    return Math.floor(m / 60) + 'h ' + (m % 60) + 'm';
  }

  // Shared with the sidebar: open an agent's conversation by its id.
  function openConversation(agentId, sessionId) {
    var store = Alpine.store('app');
    var agent = (store.agents || []).filter(function(a) { return a.id === agentId; })[0];
    if (!agent) {
      if (typeof FreecoToast !== 'undefined') FreecoToast.error('That agent is not running.');
      return;
    }
    store.pendingSession = sessionId || null;
    store.pendingAgent = agent;
    location.hash = 'agents';
  }

  document.addEventListener('alpine:init', function() {
    Alpine.data('livePage', function() {
      return {
        agents: [],
        nowMs: Date.now(),
        skewMs: 0,
        filter: 'all',
        paused: false,
        edges: [],
        chats: [],
        projectNames: {},
        loaded: false,
        error: '',
        _fast: null,
        _slow: null,
        _tick: null,
        filters: [
          { id: 'all', label: 'All' },
          { id: 'working', label: 'Working' },
          { id: 'you', label: 'Waiting for you' },
          { id: 'idle', label: 'Idle' }
        ],

        start() {
          var self = this;
          this.poll();
          this.pollSlow();
          this._fast = setInterval(function() {
            if (!self.paused && document.visibilityState === 'visible') self.poll();
          }, 1500);
          this._slow = setInterval(function() {
            if (!self.paused && document.visibilityState === 'visible') self.pollSlow();
          }, 10000);
          // Elapsed times keep counting between polls.
          this._tick = setInterval(function() {
            if (!self.paused) self.nowMs = Date.now() + self.skewMs;
          }, 1000);
        },

        destroy() {
          clearInterval(this._fast);
          clearInterval(this._slow);
          clearInterval(this._tick);
        },

        async poll() {
          try {
            var d = await FreecoAPI.get('/api/activity');
            this.agents = (d && d.agents) || [];
            // The server's clock decides what "now" is; a laptop's clock can
            // be minutes off, which would make every elapsed time wrong.
            if (d && d.now_ms) { this.skewMs = d.now_ms - Date.now(); this.nowMs = d.now_ms; }
            this.error = '';
          } catch (e) {
            this.error = 'Live updates stopped: ' + ((e && e.message) || 'no connection') + '. Retrying.';
          }
          this.loaded = true;
        },

        async pollSlow() {
          var self = this;
          try {
            var res = await Promise.all([
              FreecoAPI.get('/api/comms/topology').catch(function() { return null; }),
              FreecoAPI.get('/api/org/chats?limit=120').catch(function() { return null; }),
              FreecoAPI.get('/api/org/overview').catch(function() { return null; })
            ]);
            if (res[0]) this.edges = res[0].edges || [];
            if (res[1]) this.chats = res[1].chats || [];
            if (res[2]) {
              var names = {};
              (res[2].projects || []).forEach(function(p) { names[p.project_id] = p.project; });
              self.projectNames = names;
            }
          } catch (e) { /* the desks still work without these */ }
        },

        isWorking(a) { return a.state !== 'idle' && a.state !== 'waiting_for_you'; },

        count(id) {
          var self = this;
          return this.agents.filter(function(a) { return self.passes(a, id); }).length;
        },

        passes(a, id) {
          if (id === 'working') return this.isWorking(a);
          if (id === 'you') return a.state === 'waiting_for_you';
          if (id === 'idle') return a.state === 'idle';
          return true;
        },

        // Busy desks first (waiting for you at the very top), idle ones folded.
        get desks() {
          var self = this;
          var rank = { waiting_for_you: 0, tool: 1, waiting_on_helper: 2, thinking: 3 };
          return this.agents
            .filter(function(a) { return a.state !== 'idle' && self.passes(a, self.filter); })
            .sort(function(x, y) { return (rank[x.state] || 9) - (rank[y.state] || 9) || x.name.localeCompare(y.name); });
        },

        get idleAgents() {
          if (this.filter === 'working' || this.filter === 'you') return [];
          return this.agents.filter(function(a) { return a.state === 'idle'; });
        },

        get toolCalls() {
          var from = this.nowMs - WINDOW_MS;
          var n = 0;
          var failed = 0;
          this.agents.forEach(function(a) {
            (a.steps || []).forEach(function(s) {
              if (s.kind === 'tool' && s.started_ms >= from) { n++; if (s.ok === false) failed++; }
            });
          });
          return { n: n, failed: failed };
        },

        get waitingYou() {
          return this.agents.reduce(function(sum, a) { return sum + (a.waiting_approvals || 0); }, 0);
        },

        initials: initials,

        stateLabel(a) {
          return {
            thinking: 'thinking',
            tool: 'using a tool',
            waiting_on_helper: 'waiting on a helper',
            waiting_for_you: 'waiting for your yes',
            idle: 'idle'
          }[a.state] || a.state;
        },

        elapsed(a) {
          return a.since_ms ? span(this.nowMs - a.since_ms) : '';
        },

        nowLine(a) {
          if (a.state === 'waiting_for_you') return (a.tool ? a.tool + ' · ' : '') + 'open Waiting for you to answer';
          if (a.state === 'waiting_on_helper') return (a.tool || 'agent_delegate') + ' · ' + this.helperNames(a);
          if (a.state === 'tool') return a.tool || 'a tool';
          if (a.state === 'thinking') return a.turns > 1 ? 'working in ' + a.turns + ' chats at once' : 'working out the next step';
          var last = (a.steps || [])[a.steps.length - 1];
          return last && last.ended_ms ? 'last active ' + span(this.nowMs - last.ended_ms) + ' ago' : 'not used since the daemon started';
        },

        helperNames(a) {
          var names = this.edges
            .filter(function(e) { return e.from === a.agent_id; })
            .map(function(e) { return e.to; });
          var self = this;
          var busy = this.agents.filter(function(x) { return names.indexOf(x.agent_id) !== -1 && self.isWorking(x); });
          if (!busy.length) return 'handed work to another agent';
          return busy.map(function(x) { return x.name; }).join(', ');
        },

        askedBy(a) {
          var byId = {};
          this.agents.forEach(function(x) { byId[x.agent_id] = x.name; });
          var from = this.edges.filter(function(e) { return e.to === a.agent_id && byId[e.from]; })[0];
          return from ? 'Asked by ' + byId[from.from] : 'Asked by you';
        },

        where(a) {
          var chat = this.chatFor(a);
          if (!chat) return '';
          var project = chat.project_id ? this.projectNames[chat.project_id] : '';
          return [project, chat.label].filter(Boolean).join(' · ');
        },

        chatFor(a) {
          var mine = this.chats.filter(function(c) { return c.agent_id === a.agent_id; });
          return mine.filter(function(c) { return c.busy; })[0] || mine[0] || null;
        },

        open(a) {
          var chat = this.chatFor(a);
          openConversation(a.agent_id, chat ? chat.id : null);
        },

        stepClass(s) {
          if (s.kind === 'thinking') return 'live-s-think';
          if (HELPER_TOOLS.indexOf(s.name) !== -1) return 'live-s-help';
          if (s.ok === false) return 'live-s-fail';
          return 'live-s-tool';
        },

        lightClass(a) {
          return {
            thinking: 'live-s-think', tool: 'live-s-tool', waiting_on_helper: 'live-s-help',
            waiting_for_you: 'live-s-you', idle: 'live-s-idle'
          }[a.state] || 'live-s-idle';
        },

        lastSteps(a) {
          var s = a.steps || [];
          return s.slice(Math.max(0, s.length - 10));
        },

        // Timeline ------------------------------------------------------------

        get ticks() {
          var out = [];
          for (var i = 10; i > 0; i -= 2) {
            var d = new Date(this.nowMs - i * 60000);
            out.push((d.getHours() < 10 ? '0' : '') + d.getHours() + ':' + (d.getMinutes() < 10 ? '0' : '') + d.getMinutes());
          }
          out.push('now');
          return out;
        },

        get timelineAgents() {
          var from = this.nowMs - WINDOW_MS;
          return this.agents.filter(function(a) {
            return (a.steps || []).some(function(s) { return (s.ended_ms || Infinity) >= from; });
          });
        },

        segments(a) {
          var from = this.nowMs - WINDOW_MS;
          var now = this.nowMs;
          var self = this;
          var out = (a.steps || [])
            .filter(function(s) { return (s.ended_ms || now) >= from; })
            .map(function(s) {
              var start = Math.max(from, s.started_ms);
              var end = Math.min(now, s.ended_ms || now);
              var label = s.kind === 'thinking' ? 'thinking' : (s.name || 'tool') + (s.ok === false ? ' failed' : '');
              return {
                cls: self.stepClass(s),
                left: ((start - from) / WINDOW_MS * 100).toFixed(2),
                width: Math.max(0.4, (end - start) / WINDOW_MS * 100).toFixed(2),
                title: label + ' · ' + clock(s.started_ms) + ' · ' + span(end - s.started_ms)
              };
            });
          if (a.state === 'waiting_for_you' && a.since_ms) {
            var st = Math.max(from, a.since_ms);
            out.push({ cls: 'live-s-you', left: ((st - from) / WINDOW_MS * 100).toFixed(2), width: Math.max(0.4, (now - st) / WINDOW_MS * 100).toFixed(2), title: 'waiting for your yes' });
          }
          return out;
        },

        // Every step, newest first -------------------------------------------

        get feed() {
          var from = this.nowMs - WINDOW_MS;
          var now = this.nowMs;
          var rows = [];
          this.agents.forEach(function(a) {
            (a.steps || []).forEach(function(s) {
              if ((s.ended_ms || now) < from) return;
              var what;
              if (s.kind === 'thinking') {
                what = s.ended_ms ? 'thought for ' + span(s.ended_ms - s.started_ms) : 'thinking';
              } else if (!s.ended_ms) {
                what = (s.name || 'tool') + ' started';
              } else {
                what = (s.name || 'tool') + (s.ok === false ? ' failed' : ' ok') + ' · ' + span(s.ended_ms - s.started_ms);
              }
              rows.push({ key: a.agent_id + s.started_ms + s.kind + (s.name || ''), at: s.ended_ms || s.started_ms, time: clock(s.ended_ms || s.started_ms), agent: a.name, what: what, failed: s.ok === false });
            });
          });
          rows.sort(function(x, y) { return y.at - x.at; });
          return rows.slice(0, 40);
        },

        get whoAskedWhom() {
          var byId = {};
          this.agents.forEach(function(x) { byId[x.agent_id] = x; });
          var self = this;
          return this.edges
            .filter(function(e) { return byId[e.from] && byId[e.to]; })
            .map(function(e) {
              return {
                key: e.from + e.to + e.kind,
                from: byId[e.from].name,
                to: byId[e.to].name,
                kind: e.kind === 'parent_child' ? 'started' : 'messaged',
                live: self.isWorking(byId[e.to])
              };
            });
        }
      };
    });

    Alpine.data('artifactsPage', function() {
      return {
        rows: [],
        query: '',
        loading: false,
        loaded: false,

        async load() {
          this.loading = true;
          var agents = Alpine.store('app').agents || [];
          var lists = await Promise.all(agents.map(function(a) {
            return FreecoAPI.get('/api/agents/' + encodeURIComponent(a.id) + '/artifacts')
              .then(function(d) {
                return ((d && d.artifacts) || []).map(function(f) {
                  return Object.assign({ agent_id: a.id, agent_name: a.name }, f);
                });
              })
              .catch(function() { return []; });
          }));
          var rows = [].concat.apply([], lists);
          rows.sort(function(x, y) { return (y.modified || 0) - (x.modified || 0); });
          this.rows = rows;
          this.loading = false;
          this.loaded = true;
        },

        get visible() {
          var q = this.query.trim().toLowerCase();
          if (!q) return this.rows;
          return this.rows.filter(function(r) { return (r.path + ' ' + r.agent_name).toLowerCase().indexOf(q) !== -1; });
        },

        name(r) { return String(r.path || '').split('/').pop(); },

        folder(r) {
          var parts = String(r.path || '').split('/');
          parts.pop();
          return parts.join('/');
        },

        size(n) {
          if (!n && n !== 0) return '';
          if (n < 1024) return n + ' B';
          if (n < 1048576) return (n / 1024).toFixed(1) + ' kB';
          return (n / 1048576).toFixed(1) + ' MB';
        },

        // `modified` is seconds since the epoch.
        when(r) {
          return r.modified ? new Date(r.modified * 1000).toLocaleString() : '';
        },

        downloadUrl(r) {
          return '/api/agents/' + encodeURIComponent(r.agent_id) + '/workspace/file?download=true&path=' + encodeURIComponent(r.path);
        },

        openChat(r) { openConversation(r.agent_id, null); }
      };
    });
  });
})();
