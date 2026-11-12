// FreEco.ai — Companies, projects and teams.
//
// The structure exists in the database whether or not there is a screen for
// it, but a conversation filed under a project the user cannot see is the
// same as one filed nowhere. This is the screen that makes it real.
'use strict';

function projectsPage() {
  return {
    loading: true,
    loadError: '',
    projects: [],
    companies: [],
    overview: [],
    // Teams were being created here and never read back, because the route
    // accepted POST only. A team nobody can see is a team nobody joins.
    teams: [],
    // Archived work is hidden by default so finished projects do not crowd
    // the list, and reachable in one click so it is never lost.
    showArchived: false,
    selected: null,
    sessions: [],
    sessionsLoading: false,
    newProject: '',
    newCompany: '',
    creating: false,
    // The brief every agent working in this project reads -- the project's
    // CLAUDE.md -- and the conversations running in it right now.
    brief: '',
    briefSaving: false,
    newConvAgent: '',
    working: 0,
    _refresh: null,
    _refreshRun: 0,
    // Per team: the member being added and the goal being typed.
    teamForm: {},

    async init() {
      await this.load();
    },

    destroy() {
      if (this._refresh) { clearTimeout(this._refresh); this._refresh = null; }
      this._refreshRun++;
    },

    agents() {
      return (Alpine.store('app').agents || []);
    },

    agentName(id) {
      var a = this.agents().filter(function (x) { return x.id === id; })[0];
      return a ? a.name : '';
    },

    async load() {
      this.loading = true;
      this.loadError = '';
      try {
        var q = this.showArchived ? '?archived=true' : '';
        var results = await Promise.all([
          FreecoAPI.get('/api/org/projects' + q),
          FreecoAPI.get('/api/org/companies' + q),
          FreecoAPI.get('/api/org/overview'),
          FreecoAPI.get('/api/org/teams' + q)
        ]);
        this.projects = (results[0] && results[0].projects) || [];
        this.companies = (results[1] && results[1].companies) || [];
        this.overview = (results[2] && results[2].projects) || [];
        var teams = (results[3] && results[3].teams) || [];
        var self = this;
        teams.forEach(function (t) { self.tf(t); });
        this.teams = teams;
      } catch (e) {
        // Say what failed. A silently empty page reads as "you have no
        // projects", which is a different and more alarming message.
        this.loadError = 'Could not load projects: ' + e.message;
      }
      this.loading = false;
    },

    // Conversation counts come from the overview, which is a single query
    // rather than one per project.
    countFor(id) {
      var row = this.overview.find(function (o) { return o.project_id === id; });
      return row ? row.sessions : 0;
    },

    // Teams on a project, so the structure reads top-down: company, project,
    // the groups doing the work, and who is in them.
    teamsFor(projectId) {
      return this.teams.filter(function (t) { return t.project_id === projectId; });
    },

    companyName(id) {
      if (!id) return '';
      var c = this.companies.find(function (x) { return x.id === id; });
      return c ? c.name : '';
    },

    async createProject() {
      var name = (this.newProject || '').trim();
      if (!name) return;
      this.creating = true;
      try {
        var res = await FreecoAPI.post('/api/org/projects', { name: name });
        // The API routes rather than blindly creating, so tell the user when
        // they landed in an existing project instead of a new one.
        if (res && res.reused) {
          FreecoToast.info('That project already existed — opened it instead of making a second one.');
        } else {
          FreecoToast.success('Project created.');
        }
        this.newProject = '';
        await this.load();
      } catch (e) {
        FreecoToast.error('Could not create project: ' + e.message);
      }
      this.creating = false;
    },

    // Teams belong to a project and scope what their agents can see. Created
    // from inside a project so a team can never end up orphaned, which is the
    // state that makes scoping meaningless.
    newTeam: '',
    async createTeam() {
      var name = (this.newTeam || '').trim();
      if (!name || !this.selected) return;
      try {
        await FreecoAPI.post('/api/org/teams', { name: name, project_id: this.selected.id });
        this.newTeam = '';
        FreecoToast.success('Team added to ' + this.selected.name + '.');
        // Reload so the new team appears. Without this the only feedback was
        // the toast, and the team stayed invisible until a page refresh.
        await this.load();
      } catch (e) {
        FreecoToast.error('Could not create team: ' + e.message);
      }
    },

    tf(team) {
      if (!this.teamForm[team.id]) {
        this.teamForm[team.id] = { agent: '', role: '', goal: '', busy: false };
      }
      return this.teamForm[team.id];
    },

    // Who leads a team run: the member whose role says lead, else the first
    // to join. The same rule the daemon uses, so the page never disagrees.
    teamLead(team) {
      var ms = team.members || [];
      var lead = ms.filter(function (m) { return /lead/i.test(m.role || ''); })[0];
      return (lead || ms[0] || {}).agent_id || '';
    },

    memberLine(team) {
      var self = this;
      var lead = this.teamLead(team);
      return (team.members || []).map(function (m) {
        var name = self.agentName(m.agent_id) || String(m.agent_id).slice(0, 8);
        var role = m.role ? ' (' + m.role + ')' : '';
        var leads = m.agent_id === lead && !/lead/i.test(m.role || '') ? ' (leads)' : '';
        return name + role + leads;
      }).join(', ');
    },

    async addMember(team) {
      var f = this.tf(team);
      if (!f.agent) return;
      try {
        var res = await FreecoAPI.post('/api/org/teams/' + encodeURIComponent(team.id) + '/members',
                                       { agent_id: f.agent, role: (f.role || '').trim() });
        if (res && res.team) {
          var i = this.teams.findIndex(function (t) { return t.id === team.id; });
          if (i >= 0) this.teams.splice(i, 1, res.team);
        }
        f.agent = '';
        f.role = '';
        FreecoToast.success('Added to ' + team.name + '.');
      } catch (e) {
        FreecoToast.error('Could not add the member: ' + e.message);
      }
    },

    // The lead gets the goal in a conversation of its own and hands parts to
    // the other members, who work at the same time. The page stays here: the
    // conversations below show who is working, and each opens on its own.
    async runTeam(team) {
      var f = this.tf(team);
      var goal = (f.goal || '').trim();
      if (!goal || f.busy) return;
      f.busy = true;
      try {
        var res = await FreecoAPI.post('/api/org/teams/' + encodeURIComponent(team.id) + '/run', { goal: goal });
        f.goal = '';
        FreecoToast.success(team.name + ' started. ' + (res.agent_name || 'Its lead') +
                            ' leads; the conversations below show who is working.');
        await this.loadSessions(true);
        this.keepLive(1000);
      } catch (e) {
        FreecoToast.error('Could not start the team: ' + e.message);
      }
      f.busy = false;
    },

    async createCompany() {
      var name = (this.newCompany || '').trim();
      if (!name) return;
      try {
        await FreecoAPI.post('/api/org/companies', { name: name });
        this.newCompany = '';
        await this.load();
        FreecoToast.success('Company created.');
      } catch (e) {
        FreecoToast.error('Could not create company: ' + e.message);
      }
    },

    async select(project) {
      this.selected = project;
      this.brief = project.description || '';
      this.sessionsLoading = true;
      this.sessions = [];
      await this.loadSessions(false);
      this.sessionsLoading = false;
      this.keepLive();
    },

    // Conversations in a project run side by side; keep their state live so
    // the page shows which agents are working without a reload. While
    // something runs, look more often: a team's members start their own
    // conversations a moment after the lead, and short tasks would come and
    // go between two slow looks.
    keepLive(soon) {
      var self = this;
      if (this._refresh) clearTimeout(this._refresh);
      // A number, not an object: Alpine hands back a proxy for a stored
      // object, so an identity check against it never matches.
      var run = ++this._refreshRun;
      var tick = function (wait) {
        self._refresh = setTimeout(async function () {
          if (self.selected && !document.hidden) await self.loadSessions(true);
          if (self._refreshRun === run) tick(self.working ? 1500 : 4000);
        }, wait);
      };
      tick(soon || (this.working ? 1500 : 4000));
    },

    async loadSessions(quiet) {
      if (!this.selected) return;
      var id = this.selected.id;
      try {
        var res = await FreecoAPI.get('/api/org/projects/' + encodeURIComponent(id) + '/sessions');
        if (!this.selected || this.selected.id !== id) return;
        this.sessions = (res && res.sessions) || [];
        this.working = (res && res.working) || 0;
      } catch (e) {
        if (!quiet) FreecoToast.error('Could not load this project’s conversations: ' + e.message);
      }
    },

    async saveBrief() {
      if (!this.selected) return;
      this.briefSaving = true;
      try {
        var res = await FreecoAPI.put('/api/org/projects/' + encodeURIComponent(this.selected.id),
                                      { description: this.brief });
        if (res && res.project) {
          // Another project may have been opened while this saved.
          if (this.selected && this.selected.id === res.project.id) {
            this.selected.description = res.project.description;
          }
          var p = this.projects.find(function (x) { return x.id === res.project.id; });
          if (p) p.description = res.project.description;
        }
        FreecoToast.success('Brief saved. Agents read it on their next message in this project.');
      } catch (e) {
        FreecoToast.error('Could not save the brief: ' + e.message);
      }
      this.briefSaving = false;
    },

    // A new conversation of the chosen agent, filed here. It does not take
    // the agent away from any chat it is already in, so several run at once.
    async startConversation() {
      if (!this.selected || !this.newConvAgent) return;
      try {
        var res = await FreecoAPI.post('/api/org/projects/' + encodeURIComponent(this.selected.id) + '/conversations',
                                       { agent_id: this.newConvAgent });
        await this.loadSessions(true);
        this.openConversation({ id: res.session_id, agent_id: res.agent_id });
      } catch (e) {
        FreecoToast.error('Could not start a conversation: ' + e.message);
      }
    },

    async openConversation(s) {
      if (!s.agent_id) return;
      try {
        await FreecoAPI.post('/api/agents/' + encodeURIComponent(s.agent_id) + '/sessions/' +
                             encodeURIComponent(s.id) + '/switch', {});
      } catch (e) {
        FreecoToast.error('Could not open that conversation: ' + e.message);
        return;
      }
      window.dispatchEvent(new CustomEvent('freeco-open-agent', { detail: s.agent_id }));
    },

    // Archive is "done, keep it". Distinct from trash on purpose: if the only
    // way to clear finished work off a list is to make it look deleted,
    // people stop clearing it.
    async setArchived(session, value) {
      try {
        await FreecoAPI.put('/api/sessions/' + encodeURIComponent(session.id) + '/archive', { value: value });
        session.archived = value;
      } catch (e) {
        FreecoToast.error('Could not archive: ' + e.message);
      }
    },

    // Trash hides, it does not delete. Nothing here removes a row.
    async trash(session) {
      try {
        await FreecoAPI.put('/api/sessions/' + encodeURIComponent(session.id) + '/trash', { value: true });
        this.sessions = this.sessions.filter(function (s) { return s.id !== session.id; });
        FreecoToast.info('Moved to trash. Nothing was deleted — it can be restored.');
      } catch (e) {
        FreecoToast.error('Could not move to trash: ' + e.message);
      }
    },

    when(ts) {
      if (!ts) return '';
      return String(ts).slice(0, 16).replace('T', ' ');
    }
  };
}
