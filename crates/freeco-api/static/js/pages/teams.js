// FreEco.ai — Teams floor.
//
// Every team is a room: its members sit in it, a lit dot means a run is
// working for that agent right now, and a line between two seats is a
// hand-off the kernel recorded. Below the floor, the selected team's board:
// four columns (to do, doing, review, done) that cards are dragged between,
// and the team's members: agents, and people with a team role.
//
// Companies own teams: the switcher at the top narrows the floor to one.
// Owners and admins see every team; anyone else only the teams they are on,
// and the server decides what each may change (`access` on every team).
//
// The page shows only what the daemon can prove. It never animates work that
// is not happening, and a hand-off says "done" only when the reply came back.
'use strict';

function teamsPage() {
  return {
    loading: true,
    loadError: '',
    teams: [],
    unassigned: [],
    handoffs: [],
    selectedId: '',
    lanes: [],
    floorH: 0,
    floorW: 0,
    newTask: { title: '', description: '', assigned_to: '', blocked_by: [] },
    posting: false,
    newMember: { agent_id: '', role: '' },
    newPerson: { user: '', role: 'member' },
    accounts: [],
    newTeam: '',
    companies: [],
    companyId: '',
    manageAll: false,
    newCompany: '',
    tab: 'board',
    dragId: '',
    dropCol: '',
    columns: [
      { id: 'todo', label: 'To do' },
      { id: 'doing', label: 'Doing' },
      { id: 'review', label: 'Review' },
      { id: 'done', label: 'Done' }
    ],
    goal: '',
    running: false,
    _timer: null,
    _onResize: null,

    async init() {
      var self = this;
      if (this._timer) return;
      await this.load();
      this._timer = setInterval(function () {
        if (!document.hidden) self.load(true);
      }, 3000);
      this._onResize = function () { self.drawLanes(); };
      window.addEventListener('resize', this._onResize);
    },

    destroy() {
      if (this._timer) { clearInterval(this._timer); this._timer = null; }
      if (this._onResize) { window.removeEventListener('resize', this._onResize); this._onResize = null; }
    },

    async load(quiet) {
      if (!quiet) this.loading = true;
      try {
        var res = await FreecoAPI.get('/api/teams/floor');
        this.teams = (res && res.teams) || [];
        this.unassigned = (res && res.unassigned) || [];
        this.handoffs = (res && res.handoffs) || [];
        this.companies = (res && res.companies) || [];
        this.manageAll = !!(res && res.manage_all);
        this.loadError = '';
        var self = this;
        if (this.companyId && this.companyId !== 'none' &&
            !this.companies.some(function (c) { return c.id === self.companyId; })) this.companyId = '';
        var shown = this.visibleTeams();
        if (!this.selectedId && shown.length) this.selectedId = shown[0].id;
        if (this.selectedId && !this.selected()) this.selectedId = shown.length ? shown[0].id : '';
      } catch (e) {
        // Say what failed. An empty floor reads as "you have no teams".
        this.loadError = 'Could not load the teams: ' + e.message;
      }
      this.loading = false;
      var self = this;
      this.$nextTick(function () { self.drawLanes(); });
    },

    selected() {
      var id = this.selectedId;
      return this.visibleTeams().filter(function (t) { return t.id === id; })[0] || null;
    },

    select(t) {
      this.selectedId = t.id;
      this.newTask = { title: '', description: '', assigned_to: '', blocked_by: [] };
      this.newMember = { agent_id: '', role: '' };
      this.newPerson = { user: '', role: 'member' };
      this.accounts = [];
      this.goal = '';
      if (this.tab === 'members') this.loadAccounts();
    },

    // ── Companies ────────────────────────────────────────────────────────

    // The teams of the company picked in the switcher ('' all, 'none' the
    // teams in no company).
    visibleTeams() {
      var c = this.companyId;
      if (!c) return this.teams;
      return this.teams.filter(function (t) {
        return c === 'none' ? !t.company_id : t.company_id === c;
      });
    },

    currentCompany() {
      var c = this.companyId;
      return this.companies.filter(function (x) { return x.id === c; })[0] || null;
    },

    onCompanyChange() {
      var shown = this.visibleTeams();
      if (!this.selected()) this.selectedId = shown.length ? shown[0].id : '';
      var self = this;
      this.$nextTick(function () { self.drawLanes(); });
    },

    async createCompany() {
      var name = this.newCompany.trim();
      if (!name) return;
      try {
        var c = await FreecoAPI.post('/api/org/companies', { name: name });
        this.newCompany = '';
        if (c && c.id) this.companyId = c.id;
        await this.load(true);
      } catch (e) {
        FreecoToast.error('Could not add the company: ' + e.message);
      }
    },

    async renameCompany() {
      var c = this.currentCompany();
      if (!c) return;
      var name = (window.prompt('New name for ' + c.name, c.name) || '').trim();
      if (!name || name === c.name) return;
      try {
        await FreecoAPI.put('/api/org/companies/' + encodeURIComponent(c.id), { name: name });
        await this.load(true);
      } catch (e) {
        FreecoToast.error('Could not rename the company: ' + e.message);
      }
    },

    async deleteCompany() {
      var c = this.currentCompany();
      if (!c) return;
      if (!window.confirm('Delete ' + c.name + '? Its teams and projects stay, in no company.')) return;
      try {
        await FreecoAPI.del('/api/org/companies/' + encodeURIComponent(c.id));
        this.companyId = '';
        await this.load(true);
      } catch (e) {
        FreecoToast.error('Could not delete the company: ' + e.message);
      }
    },

    async moveTeam() {
      var t = this.selected();
      if (!t) return;
      var companyId = t.company_id || '';
      try {
        await FreecoAPI.put('/api/org/teams/' + encodeURIComponent(t.id), { company_id: companyId });
        await this.load(true);
        // Keep the team in view when it left the company being shown.
        if (!this.selected()) this.companyId = companyId || 'none';
      } catch (e) {
        FreecoToast.error('Could not move the team: ' + e.message);
      }
    },

    // ── What the caller may do (decided by the server) ───────────────────

    canManage() {
      var t = this.selected();
      return !!t && t.access === 'manage';
    },

    canWork() {
      var t = this.selected();
      return !!t && (t.access === 'manage' || t.access === 'work');
    },

    accessLabel(access) {
      if (access === 'manage') return 'You manage this team';
      if (access === 'work') return 'You work on this team';
      return 'You can read this team';
    },

    allAgents() {
      return (Alpine.store('app').agents || []);
    },

    initials(name) {
      var parts = String(name || '?').replace(/[^A-Za-z0-9 ]/g, ' ').trim().split(/\s+/);
      var s = (parts[0] || '?').charAt(0) + (parts.length > 1 ? parts[1].charAt(0) : (parts[0] || '').charAt(1));
      return s.toUpperCase();
    },

    workingNow() {
      var n = 0;
      this.teams.forEach(function (t) { t.members.forEach(function (m) { if (m.working) n++; }); });
      this.unassigned.forEach(function (m) { if (m.working) n++; });
      return n;
    },

    readyTasks() {
      return this.visibleTeams().reduce(function (sum, t) { return sum + (t.tasks.ready || 0); }, 0);
    },

    liveHandoffs() {
      return this.handoffs.filter(function (h) { return h.status === 'working'; }).length;
    },

    seatTitle(m) {
      var s = m.name + (m.role ? ' (' + m.role + ')' : '');
      if (m.working) s += ' - working in ' + m.working_in + ' conversation' + (m.working_in === 1 ? '' : 's');
      else s += ' - ' + (m.state === 'Running' ? 'idle' : m.state.toLowerCase());
      return s;
    },

    seatClass(m) {
      if (m.working) return 'seat seat-working';
      if (m.state === 'Crashed' || m.state === 'Missing') return 'seat seat-fault';
      if (m.state !== 'Running') return 'seat seat-off';
      return 'seat';
    },

    // A name for an assignee or poster: an agent id, `user:<name>` for a
    // person, or the plain "user" an earlier dashboard post recorded.
    nameOf(who) {
      if (!who) return 'someone';
      if (who === 'user') return 'you';
      if (String(who).indexOf('user:') === 0) return String(who).slice(5);
      var all = [];
      this.teams.forEach(function (t) { all = all.concat(t.members); });
      all = all.concat(this.unassigned);
      var m = all.filter(function (x) { return x.agent_id === who; })[0];
      return m ? m.name : 'someone';
    },

    ago(ts) {
      if (!ts) return '';
      var s = Math.max(0, Math.round((Date.now() - Date.parse(ts)) / 1000));
      if (s < 60) return s + 's ago';
      if (s < 3600) return Math.round(s / 60) + 'm ago';
      if (s < 86400) return Math.round(s / 3600) + 'h ago';
      return Math.round(s / 86400) + 'd ago';
    },

    // Lines between the seats of the two agents in each recent hand-off.
    // Positions are read from the rendered seats, so the floor stays plain
    // HTML that reflows on any screen and the lines follow it.
    drawLanes() {
      var floor = this.$refs && this.$refs.floor;
      if (!floor) { this.lanes = []; return; }
      var box = floor.getBoundingClientRect();
      this.floorW = Math.round(box.width);
      this.floorH = Math.round(box.height);
      var seen = {};
      var lanes = [];
      var recent = this.handoffs.slice(0, 12);
      for (var i = 0; i < recent.length; i++) {
        var h = recent[i];
        if (!h.from_agent || !h.to_agent || h.from_agent === h.to_agent) continue;
        var key = h.from_agent + '>' + h.to_agent;
        if (seen[key]) continue;
        seen[key] = true;
        var a = floor.querySelector('[data-seat="' + h.from_agent + '"]');
        var b = floor.querySelector('[data-seat="' + h.to_agent + '"]');
        if (!a || !b) continue;
        var ra = a.getBoundingClientRect();
        var rb = b.getBoundingClientRect();
        var x1 = ra.left + ra.width / 2 - box.left, y1 = ra.top + ra.height / 2 - box.top;
        var x2 = rb.left + rb.width / 2 - box.left, y2 = rb.top + rb.height / 2 - box.top;
        var mx = (x1 + x2) / 2, my = Math.min(y1, y2) - 28;
        lanes.push({
          key: key + i,
          d: 'M' + x1 + ' ' + y1 + ' Q' + mx + ' ' + my + ' ' + x2 + ' ' + y2,
          live: h.status === 'working',
          failed: h.status === 'failed',
          title: (h.from_name || 'An agent') + ' to ' + h.to_name + ': ' + h.title
        });
      }
      this.lanes = lanes;
      this.paintLanes();
    },

    // Put the lanes into the SVG. Built here rather than with x-for, which
    // cannot loop inside an <svg>; the title is set as text, never as markup.
    paintLanes() {
      var svg = this.$refs && this.$refs.lanesSvg;
      if (!svg) return;
      var ns = 'http://www.w3.org/2000/svg';
      while (svg.firstChild) svg.removeChild(svg.firstChild);
      this.lanes.forEach(function (l) {
        var path = document.createElementNS(ns, 'path');
        path.setAttribute('d', l.d);
        path.setAttribute('class', 'teams-lane' + (l.live ? ' teams-lane-live' : '') + (l.failed ? ' teams-lane-failed' : ''));
        var title = document.createElementNS(ns, 'title');
        title.textContent = l.title;
        path.appendChild(title);
        svg.appendChild(path);
      });
    },

    // The cards in one column. The server says which column each is in.
    board(column) {
      var t = this.selected();
      if (!t) return [];
      return t.board.filter(function (task) { return (task.column || 'todo') === column; });
    },

    // Everyone a card can go to: the team's agents and its people.
    assigneeOptions() {
      var t = this.selected();
      if (!t) return [];
      var out = t.members.map(function (m) { return { id: m.agent_id, label: m.name + ' (agent)' }; });
      (t.people || []).forEach(function (p) { out.push({ id: p.id, label: p.username }); });
      return out;
    },

    async changeTask(task, change, what) {
      var t = this.selected();
      if (!t) return;
      try {
        await FreecoAPI.put('/api/org/teams/' + encodeURIComponent(t.id) + '/tasks/' + encodeURIComponent(task.id), change);
        await this.load(true);
      } catch (e) {
        FreecoToast.error('Could not ' + what + ': ' + e.message);
        await this.load(true);
      }
    },

    moveTask(task, column) {
      if (!column || column === task.column) return;
      return this.changeTask(task, { status: column }, 'move the card');
    },

    assignTask(task, who) {
      if ((who || '') === (task.assigned_to || '')) return;
      return this.changeTask(task, { assigned_to: who || '' }, 'assign the card');
    },

    dropOn(column) {
      var id = this.dragId;
      this.dragId = '';
      this.dropCol = '';
      if (!id || !this.canWork()) return;
      var t = this.selected();
      var task = t && t.board.filter(function (x) { return x.id === id; })[0];
      if (task) this.moveTask(task, column);
    },

    async deleteTask(task) {
      var t = this.selected();
      if (!t || !window.confirm('Delete the card "' + task.title + '"?')) return;
      try {
        await FreecoAPI.del('/api/org/teams/' + encodeURIComponent(t.id) + '/tasks/' + encodeURIComponent(task.id));
        await this.load(true);
      } catch (e) {
        FreecoToast.error('Could not delete the card: ' + e.message);
      }
    },

    openTasks() {
      var t = this.selected();
      if (!t) return [];
      return t.board.filter(function (task) { return task.status !== 'completed'; });
    },

    taskTitle(id) {
      var t = this.selected();
      var task = t && t.board.filter(function (x) { return x.id === id; })[0];
      return task ? task.title : id.slice(0, 8);
    },

    async postTask() {
      var t = this.selected();
      var title = this.newTask.title.trim();
      if (!t || !title) return;
      this.posting = true;
      try {
        await FreecoAPI.post('/api/org/teams/' + encodeURIComponent(t.id) + '/tasks', {
          title: title,
          description: this.newTask.description.trim(),
          assigned_to: this.newTask.assigned_to || null,
          blocked_by: this.newTask.blocked_by
        });
        this.newTask = { title: '', description: '', assigned_to: '', blocked_by: [] };
        FreecoToast.success('Task added to ' + t.name + '.');
        await this.load(true);
      } catch (e) {
        FreecoToast.error('Could not add the task: ' + e.message);
      }
      this.posting = false;
    },

    async addMember() {
      var t = this.selected();
      if (!t || !this.newMember.agent_id) return;
      try {
        await FreecoAPI.post('/api/org/teams/' + encodeURIComponent(t.id) + '/members', {
          agent_id: this.newMember.agent_id,
          role: this.newMember.role || null
        });
        this.newMember = { agent_id: '', role: '' };
        await this.load(true);
      } catch (e) {
        FreecoToast.error('Could not add the agent: ' + e.message);
      }
    },

    // `id` is an agent id or `user:<name>` for a person.
    async removeMember(id, name) {
      var t = this.selected();
      if (!t) return;
      try {
        await FreecoAPI.del('/api/org/teams/' + encodeURIComponent(t.id) + '/members/' + encodeURIComponent(id));
        await this.load(true);
      } catch (e) {
        FreecoToast.error('Could not take ' + name + ' off the team: ' + e.message);
      }
    },

    openMembers() {
      this.tab = 'members';
      this.loadAccounts();
    },

    // The accounts a lead or admin may add; the server sends them only to
    // someone who manages the team.
    async loadAccounts() {
      var t = this.selected();
      if (!t || t.access !== 'manage') { this.accounts = []; return; }
      try {
        var res = await FreecoAPI.get('/api/org/teams/' + encodeURIComponent(t.id) + '/members');
        this.accounts = (res && res.accounts) || [];
      } catch (e) {
        this.accounts = [];
      }
    },

    accountsNotOnTeam() {
      var t = this.selected();
      if (!t) return [];
      var on = (t.people || []).map(function (p) { return p.username; });
      return this.accounts.filter(function (n) { return on.indexOf(n) < 0; });
    },

    async addPerson() {
      var t = this.selected();
      if (!t || !this.newPerson.user) return;
      try {
        await FreecoAPI.post('/api/org/teams/' + encodeURIComponent(t.id) + '/members', {
          user: this.newPerson.user,
          role: this.newPerson.role || 'member'
        });
        this.newPerson = { user: '', role: 'member' };
        await this.load(true);
      } catch (e) {
        FreecoToast.error('Could not add the person: ' + e.message);
      }
    },

    // The lead gets the goal in a conversation of its own and hands parts to
    // the members; the floor lights up as their runs start.
    async runTeam() {
      var t = this.selected();
      var goal = this.goal.trim();
      if (!t || !goal) return;
      this.running = true;
      try {
        await FreecoAPI.post('/api/org/teams/' + encodeURIComponent(t.id) + '/run', { goal: goal });
        this.goal = '';
        FreecoToast.success(t.name + ' is on it.');
        await this.load(true);
      } catch (e) {
        FreecoToast.error('Could not run the team: ' + e.message);
      }
      this.running = false;
    },

    async createTeam() {
      var name = this.newTeam.trim();
      if (!name) return;
      try {
        var company = this.currentCompany();
        var t = await FreecoAPI.post('/api/org/teams', { name: name, company_id: company ? company.id : null });
        this.newTeam = '';
        if (t && t.id) this.selectedId = t.id;
        await this.load(true);
      } catch (e) {
        FreecoToast.error('Could not create the team: ' + e.message);
      }
    },

    notOnTeam() {
      var t = this.selected();
      if (!t) return [];
      var ids = t.members.map(function (m) { return m.agent_id; });
      return this.allAgents().filter(function (a) { return ids.indexOf(a.id) < 0; });
    }
  };
}
