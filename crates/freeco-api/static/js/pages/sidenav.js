// FreEco.ai second sidebar: New, projects, teams and companies, tasks, chats.
//
// The chat list used to be a dropdown at the top of one agent's chat, so a
// conversation could only be found by first remembering which agent had it.
// Every chat app people know keeps its conversations in the side, newest
// first, and puts "New" above them; this is that, plus the projects, teams,
// companies and tasks an agent OS has on top.
'use strict';

document.addEventListener('alpine:init', function() {
  Alpine.data('sideNav', function() {
    return {
      newOpen: false,
      query: '',
      projects: [],
      teams: [],
      companies: [],
      tasks: [],
      chats: [],
      working: 0,
      openProjects: {},
      projectChats: {},
      activeSession: null,
      loaded: false,
      form: null,
      formBusy: false,
      _timer: null,

      init() {
        var self = this;
        try { this.openProjects = JSON.parse(localStorage.getItem('freeco-sidenav-open') || '{}') || {}; } catch (e) { this.openProjects = {}; }
        this.load();
        this._timer = setInterval(function() {
          if (document.visibilityState === 'visible') self.load();
        }, 8000);
        window.addEventListener('freeco-chat-changed', function(ev) {
          var d = (ev && ev.detail) || {};
          self.activeSession = d.sessionId || null;
          self.load();
        });
        window.addEventListener('freeco-sidenav-refresh', function() { self.load(); });
      },

      async load() {
        var get = function(path, key) {
          return FreecoAPI.get(path).then(function(d) { return (d && d[key]) || []; }).catch(function() { return null; });
        };
        var res = await Promise.all([
          get('/api/org/overview', 'projects'),
          get('/api/org/teams', 'teams'),
          get('/api/org/companies', 'companies'),
          get('/api/tasks', 'tasks'),
          get('/api/org/chats?limit=60', 'chats'),
          get('/api/activity', 'agents')
        ]);
        if (res[0]) this.projects = res[0];
        if (res[1]) this.teams = res[1];
        if (res[2]) this.companies = res[2];
        if (res[3]) this.tasks = res[3].filter(function(t) { return t.status !== 'completed'; }).slice(0, 8);
        if (res[4]) this.chats = res[4];
        if (res[5]) this.working = res[5].filter(function(a) { return a.state && a.state !== 'idle'; }).length;
        this.loaded = true;
        var self = this;
        Object.keys(this.openProjects).forEach(function(id) {
          if (self.openProjects[id]) self.loadProjectChats(id);
        });
      },

      async loadProjectChats(id) {
        try {
          var d = await FreecoAPI.get('/api/org/projects/' + encodeURIComponent(id) + '/sessions');
          this.projectChats = Object.assign({}, this.projectChats, { [id]: (d && d.sessions) || [] });
        } catch (e) { /* the project may be gone; its row disappears on the next load */ }
      },

      toggleProject(id) {
        var open = Object.assign({}, this.openProjects);
        open[id] = !open[id];
        this.openProjects = open;
        try { localStorage.setItem('freeco-sidenav-open', JSON.stringify(open)); } catch (e) { /* private window */ }
        if (open[id]) this.loadProjectChats(id);
      },

      matches(text) {
        var q = this.query.trim().toLowerCase();
        return !q || String(text || '').toLowerCase().indexOf(q) !== -1;
      },

      // Conversations not filed under a project. Project chats sit inside
      // their project, so listing them twice would only make the list longer.
      get looseChats() {
        var self = this;
        return this.chats.filter(function(c) {
          return !c.project_id && self.matches((c.label || '') + ' ' + (c.agent_name || ''));
        });
      },

      get chatGroups() {
        var groups = [];
        var byName = {};
        var startOfToday = new Date(); startOfToday.setHours(0, 0, 0, 0);
        var t0 = startOfToday.getTime();
        this.looseChats.forEach(function(c) {
          var at = Date.parse(c.updated_at || '') || 0;
          var name = at >= t0 ? 'Today' : at >= t0 - 86400000 ? 'Yesterday' : at >= t0 - 6 * 86400000 ? 'This week' : 'Earlier';
          if (!byName[name]) { byName[name] = { name: name, chats: [] }; groups.push(byName[name]); }
          byName[name].chats.push(c);
        });
        return groups;
      },

      get visibleProjects() {
        var self = this;
        return this.projects.filter(function(p) { return self.matches(p.project + ' ' + (p.company || '')); });
      },

      get visibleTasks() {
        var self = this;
        return this.tasks.filter(function(t) { return self.matches(t.title + ' ' + (t.assigned_name || '')); });
      },

      chatTitle(c) {
        return c.label || ((c.agent_name || 'Agent') + ' conversation');
      },

      initials(name) {
        var parts = String(name || '?').replace(/^freeco[-\s]/i, '').split(/[\s\-_]+/).filter(Boolean);
        return ((parts[0] || '?')[0] + ((parts[1] || '')[0] || (parts[0] || '')[1] || '')).toUpperCase();
      },

      taskLine(t) {
        if (t.status === 'in_progress') return (t.assigned_name || 'An agent') + ' is on it';
        return t.assigned_name ? 'Waiting for ' + t.assigned_name : 'Not picked up yet';
      },

      // Open a conversation where it lives: its agent's chat, switched to it.
      openChat(c) {
        var store = Alpine.store('app');
        var agent = (store.agents || []).filter(function(a) { return a.id === c.agent_id; })[0];
        if (!agent) {
          if (typeof FreecoToast !== 'undefined') FreecoToast.error('The agent for this chat is not running.');
          return;
        }
        this.activeSession = c.id;
        store.pendingSession = c.id;
        store.pendingAgent = agent;
        if (location.hash !== '#agents') location.hash = 'agents';
        store.sideNavMobile = false;
      },

      openTask(t) {
        if (!t.assigned_to) { location.hash = 'comms'; return; }
        this.openChat({ id: null, agent_id: t.assigned_to });
      },

      // New ------------------------------------------------------------------

      startNew(kind) {
        this.newOpen = false;
        if (kind === 'chat') {
          // The agent grid is the picker: choose who to talk to.
          Alpine.store('app').pendingAgent = null;
          window.dispatchEvent(new CustomEvent('close-chat'));
          location.hash = 'agents';
          return;
        }
        var agents = Alpine.store('app').agents || [];
        this.form = {
          kind: kind,
          name: '',
          description: '',
          agentId: agents.length ? agents[0].id : '',
          projectId: '',
          companyId: '',
          error: ''
        };
        var self = this;
        this.$nextTick(function() {
          var el = document.getElementById('sidenav-form-name');
          if (el) el.focus();
        });
      },

      formTitle() {
        var f = this.form || {};
        return { task: 'New task', project: 'New project', team: 'New team', company: 'New company' }[f.kind] || 'New';
      },

      async submitForm() {
        var f = this.form;
        if (!f || this.formBusy) return;
        var name = f.name.trim();
        if (!name) { f.error = f.kind === 'task' ? 'A task needs a title.' : 'It needs a name.'; return; }
        this.formBusy = true;
        f.error = '';
        try {
          if (f.kind === 'task') {
            if (!f.agentId) throw new Error('Pick the agent that should do it.');
            await FreecoAPI.post('/api/agents/' + f.agentId + '/tasks', { title: name, description: f.description.trim() });
          } else if (f.kind === 'project') {
            await FreecoAPI.post('/api/org/projects', { name: name, description: f.description.trim() || undefined, company_id: f.companyId || undefined });
          } else if (f.kind === 'team') {
            await FreecoAPI.post('/api/org/teams', { name: name, project_id: f.projectId || undefined });
          } else if (f.kind === 'company') {
            await FreecoAPI.post('/api/org/companies', { name: name, description: f.description.trim() || undefined });
          }
          if (typeof FreecoToast !== 'undefined') FreecoToast.success(this.formTitle().replace('New ', '') + ' created');
          this.form = null;
          this.load();
        } catch (e) {
          f.error = (e && e.message) || 'That did not work. Try again.';
        }
        this.formBusy = false;
      }
    };
  });
});
