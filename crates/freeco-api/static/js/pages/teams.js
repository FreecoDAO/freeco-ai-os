// FreEco.ai — Teams floor.
//
// Every team is a room: its members sit in it, a lit dot means a run is
// working for that agent right now, and a line between two seats is a
// hand-off the kernel recorded. Below the floor, the selected team's board:
// tasks that are ready, tasks waiting on other tasks, work in progress, done.
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
    newTeam: '',
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
        this.loadError = '';
        if (!this.selectedId && this.teams.length) this.selectedId = this.teams[0].id;
        if (this.selectedId && !this.selected()) this.selectedId = this.teams.length ? this.teams[0].id : '';
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
      return this.teams.filter(function (t) { return t.id === id; })[0] || null;
    },

    select(t) {
      this.selectedId = t.id;
      this.newTask = { title: '', description: '', assigned_to: '', blocked_by: [] };
      this.newMember = { agent_id: '', role: '' };
      this.goal = '';
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
      return this.teams.reduce(function (sum, t) { return sum + (t.tasks.ready || 0); }, 0);
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

    nameOf(agentId) {
      var all = [];
      this.teams.forEach(function (t) { all = all.concat(t.members); });
      all = all.concat(this.unassigned);
      var m = all.filter(function (x) { return x.agent_id === agentId; })[0];
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
    },

    board(column) {
      var t = this.selected();
      if (!t) return [];
      return t.board.filter(function (task) {
        if (column === 'ready') return task.status === 'pending' && !task.blocked;
        if (column === 'blocked') return task.blocked;
        if (column === 'in_progress') return task.status === 'in_progress';
        return task.status === 'completed';
      });
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

    async removeMember(m) {
      var t = this.selected();
      if (!t) return;
      try {
        await FreecoAPI.del('/api/org/teams/' + encodeURIComponent(t.id) + '/members/' + encodeURIComponent(m.agent_id));
        await this.load(true);
      } catch (e) {
        FreecoToast.error('Could not take the agent off the team: ' + e.message);
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
        var t = await FreecoAPI.post('/api/org/teams', { name: name });
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
