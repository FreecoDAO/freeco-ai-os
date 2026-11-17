// FreEco.ai Chat Page — Agent chat with markdown + streaming
'use strict';

function chatPage() {
  var msgId = 0;
  // Messages drawn at first, and added per "Show earlier" click. The whole
  // conversation is loaded; this only limits how much is rendered at once.
  var HISTORY_WINDOW = 200;
  var page = {
    currentAgent: null,
    messages: [],
    inputText: '',
    sending: false,
    messageQueue: [],    // Queue for messages sent while streaming
    thinkingMode: 'off', // 'off' | 'on' | 'stream'
    _wsAgent: null,
    // The conversation on screen. Every send names it, so a message lands in
    // the chat it was typed in even if another window switched the agent.
    sessionId: null,
    // Chats left while they were working, by agent id. `sending` and
    // `messageQueue` belong to the chat on screen; one shared pair of them
    // is why a reply running in one chat queued every message typed into
    // every other chat behind it, and why switching away could strand what
    // was queued for good. Now each chat keeps its own, and switching away
    // never stops, blocks or drops anything.
    _parked: {},
    _busyPoll: null,
    showSlashMenu: false,
    slashFilter: '',
    slashIdx: 0,
    attachments: [],
    dragOver: false,
    contextPressure: 'low', // green/yellow/orange/red indicator
    _typingTimeout: null,
    // Multi-session state
    sessions: [],
    sessionsOpen: false,
    // ── Conversation menu and working panel ─────────────────────────────
    //
    // Everything around a conversation besides its messages: rename it, copy
    // it into a fork, put it away, choose how the agent writes, and look at
    // what the agent actually did -- its files, what changed, what it made,
    // its sandbox, its browser, and what it runs in the background. Before,
    // all of that was reachable only by asking the agent, or not at all.
    convMenuOpen: false,
    panelOpen: false,
    panelTab: 'files',
    filesPath: '',
    filesEntries: [],
    fileView: null,
    changesList: [],
    changesHours: 24,
    artifactsList: [],
    termInput: '',
    termLines: [],
    termBusy: false,
    termTier: 'exec',
    browserShot: null,
    browserMsg: '',
    backgroundInfo: { processes: [], scheduled: [] },
    archivesList: [],
    outputStyles: [],
    // Transcript view, per browser. Tool steps shown collapsed by default.
    viewShowTools: (function () { try { return localStorage.getItem('freeco-view-tools') !== 'off'; } catch (e) { return true; } })(),
    viewTimestamps: (function () { try { return localStorage.getItem('freeco-view-ts') !== 'off'; } catch (e) { return true; } })(),
    viewCompact: (function () { try { return localStorage.getItem('freeco-view-compact') === 'on'; } catch (e) { return false; } })(),

    setView(key, on) {
      this[key] = !!on;
      var store = { viewShowTools: 'freeco-view-tools', viewTimestamps: 'freeco-view-ts', viewCompact: 'freeco-view-compact' };
      try { localStorage.setItem(store[key], on ? 'on' : 'off'); } catch (e) { /* ignore */ }
    },

    _agentUrl(path) {
      return '/api/agents/' + encodeURIComponent(this.currentAgent.id) + path;
    },

    openInNewWindow() {
      if (!this.currentAgent) return;
      window.open(window.location.origin + window.location.pathname + '?agent=' +
                  encodeURIComponent(this.currentAgent.id), '_blank', 'noopener');
      this.convMenuOpen = false;
    },

    async renameConversation() {
      this.convMenuOpen = false;
      if (!this.currentAgent) return;
      var name = window.prompt('Name this conversation:', '');
      if (name === null) return;
      try {
        await FreecoAPI.put(this._agentUrl('/conversation/label'), { label: name });
        FreecoToast.success(name.trim() ? 'Renamed' : 'Name cleared');
        this.loadSessions(this.currentAgent.id);
      } catch (e) { FreecoToast.error('Could not rename: ' + e.message); }
    },

    async forkConversation() {
      this.convMenuOpen = false;
      if (!this.currentAgent) return;
      var name = window.prompt('Name for the fork (a copy of this agent that continues from here):',
                               this.currentAgent.name + ' (fork)');
      if (name === null) return;
      try {
        var res = await FreecoAPI.post(this._agentUrl('/conversation/fork'), { name: name });
        FreecoToast.success('Forked to "' + res.name + '"');
        await Alpine.store('app').refreshAgents();
        var fork = (Alpine.store('app').agents || []).filter(function (a) { return a.id === res.agent_id; })[0];
        if (fork) this.selectAgent(fork);
      } catch (e) { FreecoToast.error('Could not fork: ' + e.message); }
    },

    archiveConversation() {
      this.convMenuOpen = false;
      if (!this.currentAgent) return;
      var self = this;
      FreecoToast.confirm('Archive conversation',
        'Put this conversation away and start fresh? It stays in the archive and can be restored.',
        async function () {
          try {
            await FreecoAPI.post(self._agentUrl('/conversation/archive'), {});
            self.messages = [];
            FreecoToast.success('Archived');
            self.loadHistory(self.currentAgent.id);
          } catch (e) { FreecoToast.error('Could not archive: ' + e.message); }
        });
    },

    deleteConversation() {
      this.convMenuOpen = false;
      if (!this.currentAgent) return;
      var self = this;
      FreecoToast.confirm('Delete conversation',
        'Clear this conversation? A copy is kept in the archive in case you change your mind.',
        async function () {
          try {
            await FreecoAPI.del(self._agentUrl('/conversation'));
            self.messages = [];
            FreecoToast.success('Deleted — a copy is in the archive');
            self.loadHistory(self.currentAgent.id);
          } catch (e) { FreecoToast.error('Could not delete: ' + e.message); }
        });
    },

    async loadArchives() {
      if (!this.currentAgent) return;
      try {
        var res = await FreecoAPI.get(this._agentUrl('/conversation/archives'));
        this.archivesList = res.archives || [];
      } catch (e) { this.archivesList = []; }
    },

    restoreArchive(a) {
      var self = this;
      FreecoToast.confirm('Restore conversation',
        'Bring back "' + (a.label || a.file) + '"? The current conversation is archived first.',
        async function () {
          try {
            await FreecoAPI.post(self._agentUrl('/conversation/archives/' + encodeURIComponent(a.file) + '/restore'), {});
            self.messages = [];
            FreecoToast.success('Restored');
            self.loadHistory(self.currentAgent.id);
            self.loadArchives();
          } catch (e) { FreecoToast.error('Could not restore: ' + e.message); }
        });
    },

    async loadOutputStyles() {
      if (this.outputStyles.length) return;
      try {
        var res = await FreecoAPI.get('/api/output-styles');
        this.outputStyles = res.styles || [];
      } catch (e) { this.outputStyles = []; }
    },

    currentOutputStyle() {
      return (this.currentAgent && this.currentAgent.output_style) || 'default';
    },

    async setOutputStyle(style) {
      if (!this.currentAgent) return;
      try {
        await FreecoAPI.put(this._agentUrl('/output-style'), { style: style });
        this.currentAgent.output_style = style;
        FreecoToast.success('Output style: ' + style);
      } catch (e) { FreecoToast.error('Could not set style: ' + e.message); }
    },

    openPanel(tab) {
      this.panelOpen = true;
      this.panelTab = tab || this.panelTab;
      this.convMenuOpen = false;
      this.refreshPanel();
    },

    refreshPanel() {
      if (!this.currentAgent) return;
      var t = this.panelTab;
      if (t === 'files') this.loadFiles(this.filesPath);
      else if (t === 'changes') this.loadChanges();
      else if (t === 'artifacts') this.loadArtifacts();
      else if (t === 'browser') this.loadBrowser();
      else if (t === 'background') this.loadBackground();
      else if (t === 'archive') this.loadArchives();
    },

    async loadFiles(path) {
      this.fileView = null;
      try {
        var res = await FreecoAPI.get(this._agentUrl('/workspace/tree?path=' + encodeURIComponent(path || '')));
        this.filesPath = res.path === '.' ? '' : (res.path || '');
        this.filesEntries = res.entries || [];
      } catch (e) { this.filesEntries = []; FreecoToast.error(e.message); }
    },

    filesUp() {
      var p = (this.filesPath || '').split('/').filter(Boolean);
      p.pop();
      this.loadFiles(p.join('/'));
    },

    async openFile(path) {
      try {
        var res = await FreecoAPI.get(this._agentUrl('/workspace/file?path=' + encodeURIComponent(path)));
        this.fileView = { path: path, content: res.content, size: res.size };
      } catch (e) {
        // Not text: offer it as a download rather than failing.
        this.fileView = { path: path, content: null };
      }
    },

    fileDownloadUrl(path) {
      return this._agentUrl('/workspace/file?download=true&path=' + encodeURIComponent(path));
    },

    async loadChanges() {
      try {
        var res = await FreecoAPI.get(this._agentUrl('/workspace/changes?hours=' + this.changesHours));
        this.changesList = res.changes || [];
      } catch (e) { this.changesList = []; }
    },

    async loadArtifacts() {
      try {
        var res = await FreecoAPI.get(this._agentUrl('/artifacts'));
        this.artifactsList = res.artifacts || [];
      } catch (e) { this.artifactsList = []; }
    },

    async runTerminal() {
      var cmd = (this.termInput || '').trim();
      if (!cmd || this.termBusy || !this.currentAgent) return;
      this.termBusy = true;
      this.termLines.push({ kind: 'cmd', text: '$ ' + cmd });
      this.termInput = '';
      try {
        var res = await FreecoAPI.post(this._agentUrl('/terminal'), { command: cmd, tier: this.termTier });
        this.termLines.push({ kind: res.ok ? 'out' : 'err', text: res.output || '' });
      } catch (e) {
        this.termLines.push({ kind: 'err', text: e.message });
      }
      if (this.termLines.length > 200) this.termLines.splice(0, this.termLines.length - 200);
      this.termBusy = false;
      this.$nextTick(function () {
        var el = document.getElementById('chat-terminal-out');
        if (el) el.scrollTop = el.scrollHeight;
      });
    },

    async loadBrowser() {
      this.browserShot = null;
      this.browserMsg = 'Loading…';
      try {
        var res = await FreecoAPI.get(this._agentUrl('/browser/screenshot'));
        if (!res.active) { this.browserMsg = res.message || 'No browser open.'; return; }
        var d = res.data || {};
        var b64 = d.image || d.screenshot || d.data || (typeof d === 'string' ? d : '');
        this.browserShot = b64 ? ('data:image/png;base64,' + b64.replace(/^data:image\/\w+;base64,/, '')) : null;
        this.browserMsg = b64 ? '' : 'The browser answered without an image.';
      } catch (e) { this.browserMsg = 'Could not reach the browser: ' + e.message; }
    },

    async loadBackground() {
      try {
        this.backgroundInfo = await FreecoAPI.get(this._agentUrl('/background'));
      } catch (e) { this.backgroundInfo = { processes: [], scheduled: [] }; }
    },

    fmtBytes(n) {
      if (!n) return '0 B';
      var u = ['B', 'KB', 'MB', 'GB'], i = 0;
      while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
      return (i ? n.toFixed(1) : n) + ' ' + u[i];
    },

    fmtAgo(secs) {
      if (!secs) return '';
      var d = Math.max(0, Date.now() / 1000 - secs);
      if (d < 60) return 'just now';
      if (d < 3600) return Math.floor(d / 60) + ' min ago';
      if (d < 86400) return Math.floor(d / 3600) + ' h ago';
      return Math.floor(d / 86400) + ' d ago';
    },

    // Read replies aloud, off by default per message but on as a mode.
    //
    // The popup had this and the main chat did not, so the same conversation
    // spoke or stayed silent depending on which window it was in. Same storage
    // key as the popup, so the choice is one setting rather than two.
    voiceOut: (function () {
      try { return localStorage.getItem('freeco-voice-out') === 'on'; } catch (e) { return false; }
    })(),
    toggleVoiceOut: function () {
      this.voiceOut = !this.voiceOut;
      try { localStorage.setItem('freeco-voice-out', this.voiceOut ? 'on' : 'off'); } catch (e) { /* private mode */ }
      if (!this.voiceOut && window.speechSynthesis) {
        try { window.speechSynthesis.cancel(); } catch (e) { /* ignore */ }
      }
    },
    // Conversation history paging. `_historyGen` rises on every load so a
    // slow reply for an agent the user has already left cannot overwrite the
    // thread they are now looking at.
    _historyGen: 0,
    _historyLimit: 0,
    historyTotal: 0,
    sessionLabel: '',
    _allHistory: [],
    _historyWindow: 200,
    historyHasMore: false,
    historyLoading: false,
    searchOpen: false,
    searchQuery: '',
    // Voice recording state
    recording: false,
    _mediaRecorder: null,
    _audioChunks: [],
    recordingTime: 0,
    _recordingTimer: null,
    // Model autocomplete state
    showModelPicker: false,
    modelPickerList: [],
    modelPickerFilter: '',
    modelPickerIdx: 0,
    // Model switcher dropdown
    showModelSwitcher: false,
    modelSwitcherFilter: '',
    modelSwitcherProviderFilter: '',
    modelSwitcherIdx: 0,
    modelSwitching: false,
    _modelCache: null,
    _modelCacheTime: 0,
    slashCommands: [], // Loaded dynamically with i18n in init()
    _slashCommandsLoaded: false,
    tokenCount: 0,

    // ── Tip Bar ──
    tipIndex: 0,
    tips: [],
    _tipsInitialized: false,
    tipTimer: null,
    get currentTip() {
      if (localStorage.getItem('of-tips-off') === 'true') return '';
      if (!this._tipsInitialized) {
        var t = typeof window.t === 'function' ? window.t : function(s) { return s; };
        this.tips = [
          t('tips.commands'),
          t('tips.think'),
          t('tips.focus'),
          'Drag files to attach',
          '/model to switch models',
          '/context to check usage',
          '/verbose off to hide tool details'
        ];
        this._tipsInitialized = true;
      }
      return this.tips[this.tipIndex % this.tips.length] || '';
    },
    dismissTips: function() { localStorage.setItem('of-tips-off', 'true'); },
    startTipCycle: function() {
      var self = this;
      if (this.tipTimer) clearInterval(this.tipTimer);
      this.tipTimer = setInterval(function() {
        self.tipIndex = (self.tipIndex + 1) % self.tips.length;
      }, 30000);
    },

    // Backward compat helper
    get thinkingEnabled() { return this.thinkingMode !== 'off'; },

    // Context pressure dot color
    get contextDotColor() {
      switch (this.contextPressure) {
        case 'critical': return '#ef4444';
        case 'high': return '#f97316';
        case 'medium': return '#eab308';
        default: return '#22c55e';
      }
    },

    get modelDisplayName() {
      if (!this.currentAgent) return '';
      var name = this.currentAgent.model_name || '';
      var short = name.replace(/-\d{8}$/, '');
      return short.length > 24 ? short.substring(0, 22) + '\u2026' : short;
    },

    get switcherProviders() {
      var seen = {};
      (this._modelCache || []).forEach(function(m) { seen[m.provider] = true; });
      return Object.keys(seen).sort();
    },

    get filteredSwitcherModels() {
      var models = this._modelCache || [];
      var provFilter = this.modelSwitcherProviderFilter;
      var textFilter = this.modelSwitcherFilter ? this.modelSwitcherFilter.toLowerCase() : '';
      if (!provFilter && !textFilter) return models;
      return models.filter(function(m) {
        if (provFilter && m.provider !== provFilter) return false;
        if (textFilter) {
          return m.id.toLowerCase().indexOf(textFilter) !== -1 ||
                 (m.display_name || '').toLowerCase().indexOf(textFilter) !== -1 ||
                 m.provider.toLowerCase().indexOf(textFilter) !== -1;
        }
        return true;
      });
    },

    get groupedSwitcherModels() {
      var filtered = this.filteredSwitcherModels;
      var groups = {}, order = [];
      filtered.forEach(function(m) {
        if (!groups[m.provider]) { groups[m.provider] = []; order.push(m.provider); }
        groups[m.provider].push(m);
      });
      return order.map(function(p) {
        return { provider: p.charAt(0).toUpperCase() + p.slice(1), models: groups[p] };
      });
    },

    // Closing the chat (back to the agent list) must not take its queue with
    // it: whatever was waiting is handed to the server, and the poll stops.
    destroy() {
      this._parkCurrentChat();
    },

    init() {
      var self = this;

      // Initialize slash commands with i18n
      this.initSlashCommands();

      // Permission requests appear in this window too, so a task blocked on one
      // is visible where the user is working rather than on another page.
      if (this.startApprovalPolling) this.startApprovalPolling(5000);

      // Start tip cycle
      this.startTipCycle();

      // Fetch dynamic commands from server
      this.fetchCommands();

      // Observe DOM for new messages and render LaTeX
      this._latexObserver = new MutationObserver(function(mutations) {
        mutations.forEach(function(mutation) {
          mutation.addedNodes.forEach(function(node) {
            if (node.nodeType === Node.ELEMENT_NODE) {
              var bubbles = node.querySelector ? node.querySelectorAll('.message-bubble') : [];
              if (node.classList && node.classList.contains('message-bubble')) {
                bubbles = [node];
              }
              bubbles.forEach(function(bubble) {
                if (bubble.textContent && hasLatexDelimiters(bubble.textContent)) {
                  renderLatex(bubble);
                }
              });
            }
          });
        });
      });
      this._latexObserver.observe(document.getElementById('messages') || document.body, {
        childList: true,
        subtree: true
      });

      // Ctrl+/ keyboard shortcut
      document.addEventListener('keydown', function(e) {
        if ((e.ctrlKey || e.metaKey) && e.key === '/') {
          e.preventDefault();
          var input = document.getElementById('msg-input');
          if (input) { input.focus(); self.inputText = '/'; }
        }
        // Ctrl+M for model switcher
        if ((e.ctrlKey || e.metaKey) && e.key === 'm' && self.currentAgent) {
          e.preventDefault();
          self.toggleModelSwitcher();
        }
        // Ctrl+F for chat search
        if ((e.ctrlKey || e.metaKey) && e.key === 'f' && self.currentAgent) {
          e.preventDefault();
          self.toggleSearch();
        }
      });

      // Load the session list when the agent changes.
      //
      // `loadSession` used to run here too, and it also assigned
      // `self.messages`. So did `loadHistory`, from `selectAgent`, against a
      // different store -- `sessions` versus `canonical_sessions`. Two async
      // loads racing for one array is why the thread sometimes came back
      // whole, sometimes truncated and sometimes empty. `canonical_sessions`
      // is what the agent itself is given, so that is the one the view must
      // show: anything else displays a conversation the model does not have.
      this.$watch('currentAgent', function(agent) {
        if (agent) {
          self.loadSessions(agent.id);
        }
      });

      // Check for pending agent from Agents page (set before chat mounted)
      var store = Alpine.store('app');
      if (store.pendingAgent) {
        self.selectAgent(store.pendingAgent);
        store.pendingAgent = null;
        self._takePendingSession();
      } else {
        // Restore previously active agent after page refresh (#1179).
        // The agent list may not be loaded yet, so resolve once it appears.
        self._restoreActiveAgent();
      }

      // Watch for future pending agent selections (e.g., user clicks agent while on chat)
      this.$watch('$store.app.pendingAgent', function(agent) {
        if (agent) {
          self.selectAgent(agent);
          Alpine.store('app').pendingAgent = null;
          self._takePendingSession();
        }
      });

      // Re-attempt restore once the agent list arrives from the server
      this.$watch('$store.app.agents', function(agents) {
        if (!self.currentAgent && agents && agents.length) {
          self._restoreActiveAgent();
        }
      });

      // Watch for slash commands + model autocomplete
      this.$watch('inputText', function(val) {
        var modelMatch = val.match(/^\/model\s+(.*)$/i);
        if (modelMatch) {
          self.showSlashMenu = false;
          self.modelPickerFilter = modelMatch[1].toLowerCase();
          if (!self.modelPickerList.length) {
            FreecoAPI.get('/api/models').then(function(data) {
              self.modelPickerList = (data.models || []).filter(function(m) { return m.available; });
              self.showModelPicker = true;
              self.modelPickerIdx = 0;
            }).catch(function() {});
          } else {
            self.showModelPicker = true;
          }
        } else if (val.startsWith('/')) {
          self.showModelPicker = false;
          self.slashFilter = val.slice(1).toLowerCase();
          self.showSlashMenu = true;
          self.slashIdx = 0;
        } else {
          self.showSlashMenu = false;
          self.showModelPicker = false;
        }
      });
    },

    get filteredModelPicker() {
      if (!this.modelPickerFilter) return this.modelPickerList.slice(0, 15);
      var f = this.modelPickerFilter;
      return this.modelPickerList.filter(function(m) {
        return m.id.toLowerCase().indexOf(f) !== -1 || (m.display_name || '').toLowerCase().indexOf(f) !== -1 || m.provider.toLowerCase().indexOf(f) !== -1;
      }).slice(0, 15);
    },

    pickModel(modelId) {
      this.showModelPicker = false;
      this.inputText = '/model ' + modelId;
      this.sendMessage();
    },

    toggleModelSwitcher() {
      if (this.showModelSwitcher) { this.showModelSwitcher = false; return; }
      var self = this;
      var now = Date.now();
      if (this._modelCache && (now - this._modelCacheTime) < 300000) {
        this.modelSwitcherFilter = '';
        this.modelSwitcherProviderFilter = '';
        this.modelSwitcherIdx = 0;
        this.showModelSwitcher = true;
        this.$nextTick(function() {
          var el = document.getElementById('model-switcher-search');
          if (el) el.focus();
        });
        return;
      }
      FreecoAPI.get('/api/models').then(function(data) {
        var models = (data.models || []).filter(function(m) { return m.available; });
        self._modelCache = models;
        self._modelCacheTime = Date.now();
        self.modelPickerList = models;
        self.modelSwitcherFilter = '';
        self.modelSwitcherProviderFilter = '';
        self.modelSwitcherIdx = 0;
        self.showModelSwitcher = true;
        self.$nextTick(function() {
          var el = document.getElementById('model-switcher-search');
          if (el) el.focus();
        });
      }).catch(function(e) {
        FreecoToast.error('Failed to load models: ' + e.message);
      });
    },

    switchModel(model) {
      if (!this.currentAgent) return;
      if (model.id === this.currentAgent.model_name) { this.showModelSwitcher = false; return; }
      var self = this;
      this.modelSwitching = true;
      var t = typeof window.t === 'function' ? window.t : function(s) { return s; };
      // Send the provider too. The server resolves a bare id through the whole
      // catalog and prefers Custom-tier entries, so `gpt-5.6-sol` could land on
      // the OpenRouter duplicate `openai/gpt-5.6-sol` and then look for
      // OPENROUTER_API_KEY -- the agent reporting a local model needing no key.
      // set_agent_model already takes an explicit provider for exactly this
      // reason (#833); the picker had it in hand and dropped it.
      FreecoAPI.put('/api/agents/' + this.currentAgent.id + '/model', { model: model.id, provider: model.provider }).then(function(resp) {
        // Use server-resolved model/provider to stay in sync (fixes #387/#466)
        self.currentAgent.model_name = (resp && resp.model) || model.id;
        self.currentAgent.model_provider = (resp && resp.provider) || model.provider;
        FreecoToast.success(t('chat.model_switched') + ' ' + (model.display_name || model.id));
        self.showModelSwitcher = false;
        self.modelSwitching = false;
      }).catch(function(e) {
        FreecoToast.error(t('chat.model_switch_failed') + ': ' + e.message);
        self.modelSwitching = false;
      });
    },

    // Initialize slash commands with i18n translations
    initSlashCommands: function() {
      if (this._slashCommandsLoaded) return;
      var t = typeof window.t === 'function' ? window.t : function(s) { return s; };
      this.slashCommands = [
        { cmd: '/help', desc: t('chat.slash.help') },
        { cmd: '/agents', desc: t('chat.slash.agents') },
        { cmd: '/new', desc: t('chat.slash.new') },
        { cmd: '/compact', desc: t('chat.slash.compact') },
        { cmd: '/model', desc: t('chat.slash.model') },
        { cmd: '/stop', desc: t('chat.slash.stop') },
        { cmd: '/usage', desc: t('chat.slash.usage') },
        { cmd: '/think', desc: t('chat.slash.think') },
        { cmd: '/context', desc: t('chat.slash.context') },
        { cmd: '/verbose', desc: t('chat.slash.verbose') },
        { cmd: '/queue', desc: t('chat.slash.queue') },
        { cmd: '/status', desc: t('chat.slash.status') },
        { cmd: '/clear', desc: t('chat.slash.clear') },
        { cmd: '/exit', desc: t('chat.slash.exit') },
        { cmd: '/budget', desc: t('chat.slash.budget') },
        { cmd: '/peers', desc: t('chat.slash.peers') },
        { cmd: '/a2a', desc: t('chat.slash.a2a') }
      ];
      this._slashCommandsLoaded = true;
    },

    // Fetch slash commands from the unified registry (/api/commands?surface=web).
    // Replaces the hardcoded initSlashCommands() list once loaded — ensures
    // the help panel and autocomplete stay in sync with the backend registry.
    fetchCommands: function() {
      var self = this;
      FreecoAPI.get('/api/commands?surface=web').then(function(data) {
        var cmds = (data && data.commands) || [];
        if (!cmds.length) return;
        self.slashCommands = cmds.map(function(c) {
          // Prefer unified-registry shape { name, aliases, description, category, requires_agent }.
          // Fall back to legacy { cmd, desc } shape so older shims keep working.
          if (c.name) {
            return {
              cmd: '/' + c.name,
              desc: c.description || '',
              category: c.category || 'general',
              aliases: c.aliases || [],
              requires_agent: !!c.requires_agent,
              source: 'registry'
            };
          }
          return {
            cmd: c.cmd,
            desc: c.desc || '',
            category: c.category || 'general',
            aliases: c.aliases || [],
            requires_agent: !!c.requires_agent,
            source: c.source || 'server'
          };
        });
        self._slashCommandsLoaded = true;
      }).catch(function() { /* silent — keep hardcoded fallback list */ });
    },

    get filteredSlashCommands() {
      if (!this.slashFilter) return this.slashCommands;
      var f = this.slashFilter;
      return this.slashCommands.filter(function(c) {
        return c.cmd.toLowerCase().indexOf(f) !== -1 || c.desc.toLowerCase().indexOf(f) !== -1;
      });
    },

    // Render `/help` output grouped by category, mirroring the
    // backend's render_help(Surfaces::WEB). Falls back to a flat list if
    // categories are not populated (pre-fetch hardcoded list).
    renderHelpText: function() {
      var order = ['general', 'session', 'model', 'control', 'memory', 'info', 'automation', 'monitoring'];
      var labels = {
        general: 'General', session: 'Session', model: 'Model', control: 'Control',
        memory: 'Memory', info: 'Info', automation: 'Automation', monitoring: 'Monitoring'
      };
      var anyCategorised = this.slashCommands.some(function(c) { return c.category; });
      if (!anyCategorised) {
        return this.slashCommands.map(function(c) {
          return '`' + c.cmd + '` \u2014 ' + c.desc;
        }).join('\n');
      }
      var groups = {};
      this.slashCommands.forEach(function(c) {
        var cat = c.category || 'general';
        if (!groups[cat]) groups[cat] = [];
        groups[cat].push(c);
      });
      var lines = ['**Available commands:**'];
      order.forEach(function(cat) {
        var list = groups[cat];
        if (!list || !list.length) return;
        lines.push('');
        lines.push('**' + (labels[cat] || cat) + '**');
        list.forEach(function(c) {
          var aliasText = '';
          if (c.aliases && c.aliases.length) {
            aliasText = ' (aliases: ' + c.aliases.map(function(a) { return '/' + a; }).join(', ') + ')';
          }
          lines.push('- `' + c.cmd + '`' + aliasText + ' \u2014 ' + c.desc);
        });
      });
      return lines.join('\n');
    },

    // Clear any stuck typing indicator after 120s
    _resetTypingTimeout: function() {
      var self = this;
      if (self._typingTimeout) clearTimeout(self._typingTimeout);
      self._typingTimeout = setTimeout(function() {
        // Auto-clear stuck typing indicators
        self.messages = self.messages.filter(function(m) { return !m.thinking; });
        self.sending = false;
        // Drain here too. Anything typed while a reply was in flight is sitting
        // in the queue, and every other place that clears `sending` drains it
        // -- these two did not. A message that lands in the queue at the wrong
        // moment stays there silently: the user presses send, the box empties,
        // and nothing ever happens. That is the worst possible failure for a
        // chat, because it is indistinguishable from being ignored.
        self._processQueue();
      }, 120000);
    },

    _clearTypingTimeout: function() {
      if (this._typingTimeout) {
        clearTimeout(this._typingTimeout);
        this._typingTimeout = null;
      }
    },

    executeSlashCommand(cmd, cmdArgs) {
      this.showSlashMenu = false;
      this.inputText = '';
      var self = this;
      cmdArgs = cmdArgs || '';
      switch (cmd) {
        case '/help':
          self.messages.push({ id: ++msgId, role: 'system', text: self.renderHelpText(), meta: '', tools: [] });
          self.scrollToBottom();
          break;
        case '/agents':
          location.hash = 'agents';
          break;
        case '/new':
          if (self.currentAgent) {
            FreecoAPI.post('/api/agents/' + self.currentAgent.id + '/session/reset', {}).then(function() {
              self.messages = [];
              self.sessionId = null; // the reset made a new conversation
              FreecoToast.success('Session reset');
            }).catch(function(e) { FreecoToast.error('Reset failed: ' + e.message); });
          }
          break;
        case '/compact':
          if (self.currentAgent) {
            self.messages.push({ id: ++msgId, role: 'system', text: 'Compacting session...', meta: '', tools: [] });
            FreecoAPI.post('/api/agents/' + self.currentAgent.id + '/session/compact', {}).then(function(res) {
              self.messages.push({ id: ++msgId, role: 'system', text: res.message || 'Compaction complete', meta: '', tools: [] });
              self.scrollToBottom();
            }).catch(function(e) { FreecoToast.error('Compaction failed: ' + e.message); });
          }
          break;
        case '/stop':
          if (self.currentAgent) {
            FreecoAPI.post('/api/agents/' + self.currentAgent.id + '/stop', {}).then(function(res) {
              self.messages.push({ id: ++msgId, role: 'system', text: res.message || 'Run cancelled', meta: '', tools: [] });
              self.sending = false;
              self._processQueue();
              self.scrollToBottom();
            }).catch(function(e) {
              // Stopping failed, so the run may still be going -- but leaving
              // `sending` true here strands every queued message behind a run
              // the user already tried to cancel.
              self.sending = false;
              self._processQueue();
              FreecoToast.error('Stop failed: ' + e.message);
            });
          }
          break;
        case '/usage':
          if (self.currentAgent) {
            var approxTokens = self.messages.reduce(function(sum, m) { return sum + Math.round((m.text || '').length / 4); }, 0);
            self.messages.push({ id: ++msgId, role: 'system', text: '**Session Usage**\n- Messages: ' + self.messages.length + '\n- Approx tokens: ~' + approxTokens, meta: '', tools: [] });
            self.scrollToBottom();
          }
          break;
        case '/think':
          if (cmdArgs === 'on') {
            self.thinkingMode = 'on';
          } else if (cmdArgs === 'off') {
            self.thinkingMode = 'off';
          } else if (cmdArgs === 'stream') {
            self.thinkingMode = 'stream';
          } else {
            // Cycle: off -> on -> stream -> off
            if (self.thinkingMode === 'off') self.thinkingMode = 'on';
            else if (self.thinkingMode === 'on') self.thinkingMode = 'stream';
            else self.thinkingMode = 'off';
          }
          var modeLabel = self.thinkingMode === 'stream' ? 'enabled (streaming reasoning)' : (self.thinkingMode === 'on' ? 'enabled' : 'disabled');
          self.messages.push({ id: ++msgId, role: 'system', text: 'Extended thinking **' + modeLabel + '**. ' +
            (self.thinkingMode === 'stream' ? 'Reasoning tokens will appear in a collapsible panel.' :
             self.thinkingMode === 'on' ? 'The agent will show its reasoning when supported by the model.' :
             'Normal response mode.'), meta: '', tools: [] });
          self.scrollToBottom();
          break;
        case '/context':
          // Send via WS command
          if (self.currentAgent && FreecoAPI.isWsConnected()) {
            FreecoAPI.wsSend({ type: 'command', command: 'context', args: '' });
          } else {
            self.messages.push({ id: ++msgId, role: 'system', text: 'Not connected (' + (FreecoAPI.getConnectionState ? FreecoAPI.getConnectionState() : 'unknown') + '). Pick an agent or check that your session is still valid.', meta: '', tools: [] });
            self.scrollToBottom();
          }
          break;
        case '/verbose':
          if (self.currentAgent && FreecoAPI.isWsConnected()) {
            FreecoAPI.wsSend({ type: 'command', command: 'verbose', args: cmdArgs });
          } else {
            self.messages.push({ id: ++msgId, role: 'system', text: 'Not connected (' + (FreecoAPI.getConnectionState ? FreecoAPI.getConnectionState() : 'unknown') + '). Pick an agent or check that your session is still valid.', meta: '', tools: [] });
            self.scrollToBottom();
          }
          break;
        case '/queue':
          if (self.currentAgent && FreecoAPI.isWsConnected()) {
            FreecoAPI.wsSend({ type: 'command', command: 'queue', args: '' });
          } else {
            self.messages.push({ id: ++msgId, role: 'system', text: 'Not connected (' + (FreecoAPI.getConnectionState ? FreecoAPI.getConnectionState() : 'unknown') + ').', meta: '', tools: [] });
            self.scrollToBottom();
          }
          break;
        case '/status':
          FreecoAPI.get('/api/status').then(function(s) {
            self.messages.push({ id: ++msgId, role: 'system', text: '**System Status**\n- Agents: ' + (s.agent_count || 0) + '\n- Uptime: ' + (s.uptime_seconds || 0) + 's\n- Version: ' + (s.version || '?'), meta: '', tools: [] });
            self.scrollToBottom();
          }).catch(function() {});
          break;
        case '/model':
          if (self.currentAgent) {
            if (cmdArgs) {
              FreecoAPI.put('/api/agents/' + self.currentAgent.id + '/model', { model: cmdArgs }).then(function(resp) {
                // Use server-resolved model/provider (fixes #387/#466)
                var resolvedModel = (resp && resp.model) || cmdArgs;
                var resolvedProvider = (resp && resp.provider) || '';
                self.currentAgent.model_name = resolvedModel;
                if (resolvedProvider) { self.currentAgent.model_provider = resolvedProvider; }
                self.messages.push({ id: ++msgId, role: 'system', text: 'Model switched to: `' + resolvedModel + '`' + (resolvedProvider ? ' (provider: `' + resolvedProvider + '`)' : ''), meta: '', tools: [] });
                self.scrollToBottom();
              }).catch(function(e) { FreecoToast.error('Model switch failed: ' + e.message); });
            } else {
              self.messages.push({ id: ++msgId, role: 'system', text: '**Current Model**\n- Provider: `' + (self.currentAgent.model_provider || '?') + '`\n- Model: `' + (self.currentAgent.model_name || '?') + '`', meta: '', tools: [] });
              self.scrollToBottom();
            }
          } else {
            self.messages.push({ id: ++msgId, role: 'system', text: 'No agent selected.', meta: '', tools: [] });
            self.scrollToBottom();
          }
          break;
        case '/clear':
          self.messages = [];
          break;
        case '/exit':
          FreecoAPI.wsDisconnect();
          self._wsAgent = null;
          self.currentAgent = null;
          self.messages = [];
          try { localStorage.removeItem('of-active-agent'); } catch(e) { /* ignore */ }
          window.dispatchEvent(new Event('close-chat'));
          break;
        case '/budget':
          FreecoAPI.get('/api/budget').then(function(b) {
            var fmt = function(v) { return v > 0 ? '$' + v.toFixed(2) : 'unlimited'; };
            self.messages.push({ id: ++msgId, role: 'system', text: '**Budget Status**\n' +
              '- Hourly: $' + (b.hourly_spend||0).toFixed(4) + ' / ' + fmt(b.hourly_limit) + '\n' +
              '- Daily: $' + (b.daily_spend||0).toFixed(4) + ' / ' + fmt(b.daily_limit) + '\n' +
              '- Monthly: $' + (b.monthly_spend||0).toFixed(4) + ' / ' + fmt(b.monthly_limit), meta: '', tools: [] });
            self.scrollToBottom();
          }).catch(function() {});
          break;
        case '/peers':
          FreecoAPI.get('/api/network/status').then(function(ns) {
            self.messages.push({ id: ++msgId, role: 'system', text: '**OFP Network**\n' +
              '- Status: ' + (ns.enabled ? 'Enabled' : 'Disabled') + '\n' +
              '- Connected peers: ' + (ns.connected_peers||0) + ' / ' + (ns.total_peers||0), meta: '', tools: [] });
            self.scrollToBottom();
          }).catch(function() {});
          break;
        case '/a2a':
          FreecoAPI.get('/api/a2a/agents').then(function(res) {
            var agents = res.agents || [];
            if (!agents.length) {
              self.messages.push({ id: ++msgId, role: 'system', text: 'No external A2A agents discovered.', meta: '', tools: [] });
            } else {
              var lines = agents.map(function(a) { return '- **' + a.name + '** — ' + a.url; });
              self.messages.push({ id: ++msgId, role: 'system', text: '**A2A Agents (' + agents.length + ')**\n' + lines.join('\n'), meta: '', tools: [] });
            }
            self.scrollToBottom();
          }).catch(function() {});
          break;
      }
    },

    // Restore the previously-active agent (set in selectAgent) after a page
    // refresh, so the WebSocket re-attaches to the same session and any
    // in-flight tool output streams back into the chat (#1179).
    _restoreActiveAgent: function() {
      var storedId = null;
      try { storedId = localStorage.getItem('of-active-agent'); } catch(e) { /* ignore */ }
      if (!storedId) return;
      var agents = (Alpine.store('app') && Alpine.store('app').agents) || [];
      var match = null;
      for (var i = 0; i < agents.length; i++) {
        if (agents[i] && agents[i].id === storedId) { match = agents[i]; break; }
      }
      if (match) {
        this.selectAgent(match);
      }
    },

    // Render an agent reply exactly once.
    //
    // The same answer can arrive twice: the HTTP fallback renders a reply, and
    // a WebSocket that reconnects mid-turn then delivers the completion for the
    // same run. Both paths are individually correct, which is why this is
    // deduplicated where they meet rather than in either of them.
    //
    // Matching is on the text: two different runs producing identical text in
    // the same second are the same answer as far as a reader is concerned, and
    // showing it twice is worse than dropping a genuine repeat.
    _pushAgentReply: function (text, meta, tools, ts) {
      var body = (text || '').trim();
      if (this.voiceOut && body) { this.speak(body); }
      // The turn produced an answer, so the user's message is now part of the
      // server's history and no longer needs carrying across a reload.
      this.messages.forEach(function(m) { if (m.pending) m.pending = false; });
      for (var i = this.messages.length - 1; i >= 0 && i >= this.messages.length - 4; i--) {
        var m = this.messages[i];
        if (m.role !== 'agent' || m.thinking || m.streaming) continue;
        if ((m.text || '').trim() === body && body !== '') {
          // Keep the richer metadata if this copy has it.
          if (!m.meta && meta) m.meta = meta;
          if ((!m.tools || !m.tools.length) && tools && tools.length) m.tools = tools;
          return m;
        }
        break;
      }
      var msg = {
        id: ++msgId, role: 'agent', text: text, meta: meta || '',
        tools: tools || [], ts: ts || Date.now()
      };
      this.messages.push(msg);
      return msg;
    },

    // --- Per-message actions ---

    // Read an answer aloud. Toggles, so a long reply can be stopped.
    speakMessage: function (m) {
      if (!window.speechSynthesis) { FreecoToast.error('This browser cannot speak.'); return; }
      if (window.speechSynthesis.speaking) { window.speechSynthesis.cancel(); return; }
      this.speak(this.plainText(m));
    },

    copyMessage: function (m) {
      var t = this.plainText(m);
      if (navigator.clipboard) {
        navigator.clipboard.writeText(t).then(function () { FreecoToast.success('Copied'); },
                                              function () { FreecoToast.error('Copy failed'); });
      }
    },

    // Save the answer as a file. Markdown, because that is what it is written
    // in -- saving rendered HTML would lose the source the user can edit.
    saveMessage: function (m) {
      try {
        var name = 'freeco-' + new Date(m.ts || Date.now()).toISOString().replace(/[:.]/g, '-') + '.md';
        var blob = new Blob([this.plainText(m)], { type: 'text/markdown' });
        var url = URL.createObjectURL(blob);
        var a = document.createElement('a');
        a.href = url; a.download = name;
        document.body.appendChild(a); a.click(); document.body.removeChild(a);
        setTimeout(function () { URL.revokeObjectURL(url); }, 1000);
      } catch (e) { FreecoToast.error('Could not save the file.'); }
    },

    // Hand this answer to another agent, with a line saying where it came from
    // -- an excerpt arriving with no provenance is a puzzle for the receiver.
    sendMessageTo: function (m) {
      this.sendToText = this.plainText(m);
      this.sendToOpen = true;
    },
    sendToOpen: false,
    sendToText: '',
    sendToBusy: false,

    confirmSendTo: async function (agentId) {
      if (!agentId || this.sendToBusy) return;
      this.sendToBusy = true;
      try {
        var from = (this.currentAgent && this.currentAgent.name) || 'another agent';
        await FreecoAPI.post('/api/agents/' + encodeURIComponent(agentId) + '/message',
                             { message: 'Forwarded from ' + from + ':\n\n' + this.sendToText });
        FreecoToast.success('Sent');
        this.sendToOpen = false;
      } catch (e) {
        FreecoToast.error('Could not send: ' + (e.message || 'unknown error'));
      }
      this.sendToBusy = false;
    },

    // The text as written, not as rendered.
    plainText: function (m) {
      return String((m && m.text) || '');
    },

    // --- Enter behaviour: one Enter is a newline, two in a row send ---
    //
    // A single Enter used to send, which makes a multi-line message impossible
    // to write: every paragraph break fires the message half-written. Shift+Enter
    // was the only way to get a newline, which is a convention people who write
    // code know and people who write sentences do not.
    //
    // The rule now: Enter inserts a newline, and a second Enter immediately
    // after sends. The window is deliberate -- a pause between them means the
    // user is composing, not finishing.
    _lastEnterAt: 0,

    enterSends: function (ev) {
      var now = Date.now();
      var doubled = (now - this._lastEnterAt) < 1200;
      this._lastEnterAt = now;
      if (!doubled) return false;
      // The first Enter already inserted a newline; drop it so the sent text
      // does not end in a blank line.
      this.inputText = (this.inputText || '').replace(/\n[ \t]*$/, '');
      this._lastEnterAt = 0;
      return true;
    },

    // --- Voice, matching the floating assistant ---
    //
    // The chat page markup already referenced `recording`, so the controls were
    // designed for and never implemented here: dictation existed in the
    // floating widget only, and the same product behaved differently depending
    // on which window you happened to be in.
    //
    // Transcribed speech lands in the input for review rather than sending
    // itself. Recognition mishears, and a misheard message sent automatically
    // is one the user has to notice, retract and correct.
    recording: false,
    _recognition: null,

    toggleVoice: function () {
      var Rec = window.SpeechRecognition || window.webkitSpeechRecognition;
      var cfg = window.FreecoVoiceCfg;
      // No recogniser, or one already proven unusable here: record and let the
      // server transcribe instead of refusing outright. Brave defines the
      // object and removed the service behind it, so "has speech recognition"
      // was true while dictation could never work.
      if (!Rec || (cfg && cfg.browserSttBroken())) {
        this.startRecording();
        return;
      }
      if (this.recording) { this.stopVoice(); return; }

      var self = this;
      var rec = new Rec();
      rec.lang = (cfg && cfg.lang()) || navigator.language || 'en-US';
      rec.interimResults = true;
      rec.continuous = true;

      var settled = '';
      rec.onresult = function (ev) {
        var interim = '';
        for (var i = ev.resultIndex; i < ev.results.length; i++) {
          var chunk = ev.results[i][0].transcript;
          if (ev.results[i].isFinal) { settled += chunk; } else { interim += chunk; }
        }
        // Shown live so the user can see it being heard, and correct it before
        // anything is sent.
        self.inputText = (settled + interim).trim();
      };
      rec.onerror = function (e) {
        self.recording = false;
        var why = (e && e.error) || 'unknown';
        var vc = window.FreecoVoiceCfg;
        if (vc && vc.isPermanentSttError(why)) {
          // The same failure would repeat on every press, so switch paths and
          // do the thing the user asked for rather than reporting a code.
          vc.markBrowserSttBroken();
          FreecoToast.info('This browser cannot transcribe ' + rec.lang +
            ' itself — recording instead, and FreEco.ai will transcribe it.');
          self.startRecording();
          return;
        }
        if (why !== 'aborted') FreecoToast.error('Voice input failed: ' + why);
      };
      rec.onend = function () { self.recording = false; };

      this._recognition = rec;
      this.recording = true;
      try { rec.start(); } catch (e) { this.recording = false; }
    },

    stopVoice: function () {
      this.recording = false;
      if (this._recognition) {
        try { this._recognition.stop(); } catch (e) { /* already stopped */ }
        this._recognition = null;
      }
    },

    // Read a reply aloud, as the floating widget does.
    speak: function (text) {
      if (!window.speechSynthesis || !text) return;
      var cfg = window.FreecoVoiceCfg;
      // Markdown stripped first: read raw, "**" was spoken as "asterisk asterisk".
      var clean = cfg ? cfg.cleanForSpeech(text) : String(text);
      if (!clean) return;
      try {
        window.speechSynthesis.cancel();
        // Same language as dictation: a reply read in the wrong language is
        // unintelligible even when the text is right.
        var lang = (cfg && cfg.lang()) || navigator.language || 'en-US';
        // The voice chosen in the assistant, or the best local one -- not the
        // browser default, which is the metallic one.
        var voice = cfg ? cfg.pickVoice() : null;
        // Sentence-sized pieces: Chrome stops one long utterance after ~15 s.
        var chunks = clean.slice(0, 8000).match(/[^.!?]{1,180}([.!?]+|$)\s*/g) || [clean];
        chunks.forEach(function (part) {
          var u = new SpeechSynthesisUtterance(part.trim());
          u.lang = lang;
          if (voice) u.voice = voice;
          window.speechSynthesis.speak(u);
        });
      } catch (e) { /* speaking is optional, never fatal */ }
    },

    // Load what the agent still remembers, so the view matches the model's.
    //
    // The summary line is deliberate: once a conversation outgrows the replay
    // window the older turns are compacted rather than kept verbatim, and
    // saying so is the difference between "it forgot" and "it summarised".
    // Load the conversation the agent is actually in -- its current session,
    // the store every message is written to -- with tool steps and images.
    //
    // This read `/history` (the agent's compact cross-channel memory) and
    // built messages with `html` and role `assistant`, while the template
    // draws `text` with role `agent`. So the history arrived and was never
    // drawn: whole conversations looked empty, and "load earlier" fetched more
    // of the wrong store. The session is the conversation; it is what is shown.
    //
    // Everything is fetched once and the last HISTORY_WINDOW messages drawn;
    // "Show earlier" reveals more from memory, with no request that can fail.
    async loadHistory(agentId, sessionId) {
      var gen = ++this._historyGen;
      this.historyLoading = true;
      try {
        // Without a conversation named, the agent's current one.
        var url = '/api/agents/' + encodeURIComponent(agentId) + '/session';
        if (sessionId) url += '?session_id=' + encodeURIComponent(sessionId);
        var data = await FreecoAPI.get(url);
        // A newer load started while this one was in flight -- that agent or
        // session is what the user is looking at now.
        if (gen !== this._historyGen || !data) return;
        var self = this;
        var all = (data.messages || [])
          .filter(function(m) { return m && m.role !== 'System' && m.role !== 'system'; })
          .map(function(m) { return self._fromStored(m); })
          .filter(function(m) { return (m.text && m.text.trim()) || m.tools.length || m.images.length; });
        this._allHistory = all;
        this.sessionLabel = data.label || '';
        this.sessionId = data.session_id || null;
        this.historyTotal = all.length;
        this._historyWindow = HISTORY_WINDOW;
        this._showHistoryWindow();
        // A parked chat whose work is all done and saved is just history now.
        var key = this._parkKey(agentId, this.sessionId);
        var parked = this._parked[key];
        if (parked && !parked.sending && !data.busy) { delete this._parked[key]; parked = null; }
        if (parked) this._showParkedMessages(parked);
        this.$nextTick(function() { self.scrollToBottom(true); });
        // A run is still working here: left mid-reply, a message delivered
        // while away, or a reload during a run. Say so and pick the reply up
        // when it lands, rather than show a thread that looks ignored.
        if (data.busy || parked) this._watchBusy(agentId, this.sessionId);
      } catch (e) {
        if (gen === this._historyGen) {
          FreecoToast.error('Could not load this conversation: ' + (e.message || 'error'));
        }
      } finally {
        if (gen === this._historyGen) this.historyLoading = false;
      }
    },

    // Draw the newest `_historyWindow` stored messages, keeping anything typed
    // but not yet answered: it is not stored until its turn finishes, and
    // replacing the list outright is what made a just-sent message vanish.
    _showHistoryWindow() {
      var all = this._allHistory || [];
      var start = Math.max(0, all.length - this._historyWindow);
      this.historyHasMore = start > 0;
      var pending = this.messages.filter(function(m) {
        return m.pending || m.streaming || m.thinking;
      });
      this.messages = all.slice(start).concat(pending);
    },

    // One mapping from a stored message to what the template draws, used by
    // every loader, so two loaders can never disagree about the shape again.
    _fromStored(m) {
      var role = (m.role === 'User' || m.role === 'user') ? 'user'
        : (m.role === 'System' || m.role === 'system') ? 'system' : 'agent';
      var text = typeof m.content === 'string' ? m.content : JSON.stringify(m.content || '');
      text = this.sanitizeToolText(text);
      var tools = (m.tools || []).map(function(t, idx) {
        return {
          id: (t.name || 'tool') + '-hist-' + (++msgId) + '-' + idx,
          name: t.name || 'unknown',
          running: false,
          // Finished work from history loads collapsed; click to open.
          expanded: false,
          input: t.input || '',
          result: t.result || '',
          is_error: !!t.is_error
        };
      });
      var images = (m.images || []).map(function(img) {
        return { file_id: img.file_id, filename: img.filename || 'image' };
      });
      return { id: ++msgId, role: role, text: text, meta: '', tools: tools, images: images };
    },

    // Show more of the conversation. Instant: it is already loaded.
    loadEarlierHistory() {
      if (!this.currentAgent || this.historyLoading) return;
      var el = document.getElementById('messages');
      var before = el ? el.scrollHeight - el.scrollTop : 0;
      this._historyWindow += HISTORY_WINDOW;
      this._showHistoryWindow();
      // Keep the message the user was reading where it was.
      this.$nextTick(function() { if (el) el.scrollTop = el.scrollHeight - before; });
    },

    selectAgent(agent) {
      // Another chat: park this one with its run and queue intact, and start
      // the next with its own. The same chat again keeps both.
      if (!this.currentAgent || this.currentAgent.id !== agent.id) {
        if (this.currentAgent) this._parkCurrentChat();
        this.sessionId = null;
        this.sending = false;
        this.messageQueue = [];
      }
      this.currentAgent = agent;
      this.messages = [];
      try { localStorage.setItem('of-active-agent', agent.id); } catch(e) { /* ignore */ }
      // Put the conversation back.
      //
      // Clearing the view without reloading it is why messages "vanished"
      // after switching agent or model: the backend still held every one of
      // them, and `/api/agents/{id}/history` had existed all along with nothing
      // calling it. The user saw an empty thread and reasonably concluded the
      // history was gone, while the agent still had it -- the two disagreeing
      // about what was said is worse than either being empty.
      var historyLoaded = this.loadHistory(agent.id);
      this.connectWs(agent.id);
      var t = typeof window.t === 'function' ? window.t : function(s) { return s; };
      // Show welcome tips on first use.
      //
      // These are appended after the history resolves, not before it. Pushing
      // them first made the thread non-empty, and the old restore only ran
      // `if (!this.messages.length)` -- so on the one chat where the tips
      // appear, the entire conversation was skipped, and the flag was set so
      // it never recurred and never got explained.
      // An untranslated key is not a tip: it was shown raw ("chat.welcome_message").
      if (!localStorage.getItem('of-chat-tips-seen') && t('chat.welcome_message') !== 'chat.welcome_message') {
        var self0 = this;
        Promise.resolve(historyLoaded).then(function() {
          self0.messages.push({
            id: ++msgId,
            role: 'system',
            text: t('chat.welcome_message'),
            meta: '',
            tools: []
          });
          self0.scrollToBottom();
        });
        localStorage.setItem('of-chat-tips-seen', 'true');
      }
      // Focus input after agent selection
      var self = this;
      this.$nextTick(function() {
        var el = document.getElementById('msg-input');
        if (el) el.focus();
      });
    },

    // The current session, through the one loader the chat uses.
    async loadSession(agentId) {
      return this.loadHistory(agentId);
    },

    // Multi-session: load session list for current agent
    async loadSessions(agentId) {
      try {
        var data = await FreecoAPI.get('/api/agents/' + agentId + '/sessions');
        this.sessions = data.sessions || [];
      } catch(e) { this.sessions = []; }
      // The sidebar marks the open conversation and refreshes its list.
      var active = this.sessions.filter(function(s) { return s.active; })[0];
      window.dispatchEvent(new CustomEvent('freeco-chat-changed', {
        detail: { agentId: agentId, sessionId: active ? active.session_id : null }
      }));
    },

    // A conversation picked in the sidebar: the agent is selected first, then
    // this switches to the conversation itself unless it is already open.
    async _takePendingSession() {
      var store = Alpine.store('app');
      var sid = store.pendingSession;
      store.pendingSession = null;
      if (!sid || !this.currentAgent) return;
      await this.loadSessions(this.currentAgent.id);
      var target = this.sessions.filter(function(s) { return s.session_id === sid; })[0];
      if (target && !target.active) await this.switchSession(sid);
    },

    // Multi-session: create a new session
    async createSession() {
      if (!this.currentAgent) return;
      var t = typeof window.t === 'function' ? window.t : function(s) { return s; };
      var label = prompt(t('chat.session_name_prompt'));
      if (label === null) return; // cancelled
      this._leaveConversation();
      try {
        await FreecoAPI.post('/api/agents/' + this.currentAgent.id + '/sessions', {
          label: label.trim() || undefined
        });
        await this.loadSessions(this.currentAgent.id);
        await this.loadSession(this.currentAgent.id);
        this.messages = [];
        this.scrollToBottom();
        this._wsAgent = null;
        this.connectWs(this.currentAgent.id);
        if (typeof FreecoToast !== 'undefined') FreecoToast.success(t('chat.session_created'));
      } catch(e) {
        if (typeof FreecoToast !== 'undefined') FreecoToast.error(t('chat.session_create_failed'));
      }
    },

    // Multi-session: switch to an existing session
    async switchSession(sessionId) {
      if (!this.currentAgent) return;
      this._leaveConversation();
      try {
        await FreecoAPI.post('/api/agents/' + this.currentAgent.id + '/sessions/' + sessionId + '/switch', {});
        this.messages = [];
        await this.loadSession(this.currentAgent.id);
        await this.loadSessions(this.currentAgent.id);
        // Reconnect WebSocket for new session
        this._wsAgent = null;
        this.connectWs(this.currentAgent.id);
      } catch(e) {
        if (typeof FreecoToast !== 'undefined') FreecoToast.error('Failed to switch session');
      }
    },

    connectWs(agentId) {
      if (this._wsAgent === agentId) return;
      this._wsAgent = agentId;
      var self = this;

      FreecoAPI.wsConnect(agentId, {
        onOpen: function() {
          Alpine.store('app').wsConnected = true;
        },
        onMessage: function(data) { self.handleWsMessage(data); },
        onClose: function() {
          Alpine.store('app').wsConnected = false;
          self._wsAgent = null;
        },
        onError: function() {
          Alpine.store('app').wsConnected = false;
          self._wsAgent = null;
        }
      });
    },

    handleWsMessage(data) {
      switch (data.type) {
        case 'connected': break;

        // Incoming message from server (e.g., cron trigger) — display as user message
        case 'message':
          if (data.content) {
            var meta = data.source === 'cron' ? '[Scheduled: ' + (data.job_name || data.job_id || '') + ']' : '';
            this.messages.push({ id: ++msgId, role: 'user', text: data.content, meta: meta, tools: [], images: [], ts: Date.now() });
            this.scrollToBottom();
          }
          break;

        // Legacy thinking event (backward compat)
        case 'thinking':
          if (!this.messages.length || !this.messages[this.messages.length - 1].thinking) {
            var thinkLabel = data.level ? 'Thinking (' + data.level + ')...' : 'Processing...';
            this.messages.push({ id: ++msgId, role: 'agent', text: thinkLabel, meta: '', thinking: true, streaming: true, tools: [] });
            this.scrollToBottom();
            this._resetTypingTimeout();
          } else if (data.level) {
            var thinkIdx = this.messages.length - 1;
            var lastThink = thinkIdx >= 0 ? this.messages[thinkIdx] : null;
            if (lastThink && lastThink.thinking) {
              lastThink.text = 'Thinking (' + data.level + ')...';
              this.messages.splice(thinkIdx, 1, lastThink);
            }
          }
          break;

        // New typing lifecycle
        case 'typing':
          if (data.state === 'start') {
            if (!this.messages.length || !this.messages[this.messages.length - 1].thinking) {
              this.messages.push({ id: ++msgId, role: 'agent', text: 'Processing...', meta: '', thinking: true, streaming: true, tools: [] });
              this.scrollToBottom();
            }
            this._resetTypingTimeout();
          } else if (data.state === 'tool') {
            var toolTypIdx = this.messages.length - 1;
            var typingMsg = toolTypIdx >= 0 ? this.messages[toolTypIdx] : null;
            if (typingMsg && (typingMsg.thinking || typingMsg.streaming)) {
              typingMsg.text = 'Using ' + (data.tool || 'tool') + '...';
              this.messages.splice(toolTypIdx, 1, typingMsg);
            }
            this._resetTypingTimeout();
          } else if (data.state === 'stop') {
            this._clearTypingTimeout();
          }
          break;

        case 'phase':
          // Show tool/phase progress so the user sees the agent is working
          var phaseIdx = this.messages.length - 1;
          var phaseMsg = phaseIdx >= 0 ? this.messages[phaseIdx] : null;
          if (phaseMsg && (phaseMsg.thinking || phaseMsg.streaming)) {
            // Skip phases that have no user-meaningful display text — "streaming"
            // and "done" are lifecycle signals, not status to show in the chat bubble.
            if (data.phase === 'streaming' || data.phase === 'done') {
              break;
            }
            // Context warning: show prominently as a separate system message
            if (data.phase === 'context_warning') {
              var cwDetail = data.detail || 'Context limit reached.';
              this.messages.push({ id: ++msgId, role: 'system', text: cwDetail, meta: '', tools: [] });
            } else if (data.phase === 'thinking' && this.thinkingMode === 'stream') {
              // Stream reasoning tokens to a collapsible panel
              if (!phaseMsg._reasoning) phaseMsg._reasoning = '';
              phaseMsg._reasoning += (data.detail || '') + '\n';
              phaseMsg.text = '<details><summary>Reasoning...</summary>\n\n' + phaseMsg._reasoning + '</details>';
              this.messages.splice(phaseIdx, 1, phaseMsg);
            } else if (phaseMsg.thinking) {
              // Only update text on messages still in thinking state (not yet
              // receiving streamed content) to avoid overwriting accumulated text.
              var phaseDetail;
              if (data.phase === 'tool_use') {
                phaseDetail = 'Using ' + (data.detail || 'tool') + '...';
              } else if (data.phase === 'thinking') {
                phaseDetail = 'Thinking...';
              } else {
                phaseDetail = data.detail || 'Working...';
              }
              phaseMsg.text = phaseDetail;
              this.messages.splice(phaseIdx, 1, phaseMsg);
            }
          }
          this.scrollToBottom();
          break;

        case 'text_delta':
          var lastIdx = this.messages.length - 1;
          var last = lastIdx >= 0 ? this.messages[lastIdx] : null;
          if (last && last.streaming) {
            if (last.thinking) { last.text = ''; last.thinking = false; }
            // If we already detected a text-based tool call, skip further text
            if (last._toolTextDetected) break;
            last.text += data.content;
            // Detect function-call patterns streamed as text and convert to tool cards
            var fcIdx = last.text.search(/\w+<\/function[=,>]/);
            if (fcIdx === -1) fcIdx = last.text.search(/<function=\w+>/);
            if (fcIdx !== -1) {
              var fcPart = last.text.substring(fcIdx);
              var toolMatch = fcPart.match(/^(\w+)<\/function/) || fcPart.match(/^<function=(\w+)>/);
              last.text = last.text.substring(0, fcIdx).trim();
              last._toolTextDetected = true;
              if (toolMatch) {
                if (!last.tools) last.tools = [];
                var inputMatch = fcPart.match(/[=,>]\s*(\{[\s\S]*)/);
                last.tools.push({
                  id: toolMatch[1] + '-txt-' + Date.now(),
                  name: toolMatch[1],
                  running: true,
                  expanded: true,
                  input: inputMatch ? inputMatch[1].replace(/<\/function>?\s*$/, '').trim() : '',
                  result: '',
                  is_error: false
                });
              }
            }
            this.tokenCount = Math.round(last.text.length / 4);
            // Force Alpine reactivity: splice-in-place so x-for re-renders
            // this item. Direct property mutation on array elements may not
            // trigger DOM updates from async WebSocket callbacks.
            this.messages.splice(lastIdx, 1, last);
          } else {
            this.messages.push({ id: ++msgId, role: 'agent', text: data.content, meta: '', streaming: true, tools: [] });
          }
          this.scrollToBottom();
          break;

        case 'tool_start':
          var tsIdx = this.messages.length - 1;
          var lastMsg = tsIdx >= 0 ? this.messages[tsIdx] : null;
          if (lastMsg && lastMsg.streaming) {
            if (!lastMsg.tools) lastMsg.tools = [];
            lastMsg.tools.push({ id: data.tool + '-' + Date.now(), name: data.tool, running: true, expanded: true, input: '', result: '', is_error: false });
            this.messages.splice(tsIdx, 1, lastMsg);
          }
          this.scrollToBottom();
          break;

        case 'tool_end':
          // Tool call parsed by LLM — update tool card with input params
          var teIdx = this.messages.length - 1;
          var lastMsg2 = teIdx >= 0 ? this.messages[teIdx] : null;
          if (lastMsg2 && lastMsg2.tools) {
            for (var ti = lastMsg2.tools.length - 1; ti >= 0; ti--) {
              if (lastMsg2.tools[ti].name === data.tool && lastMsg2.tools[ti].running) {
                lastMsg2.tools[ti].input = data.input || '';
                break;
              }
            }
            this.messages.splice(teIdx, 1, lastMsg2);
          }
          break;

        case 'tool_result':
          // Tool execution completed — update tool card with result
          var trIdx = this.messages.length - 1;
          var lastMsg3 = trIdx >= 0 ? this.messages[trIdx] : null;
          if (lastMsg3 && lastMsg3.tools) {
            for (var ri = lastMsg3.tools.length - 1; ri >= 0; ri--) {
              if (lastMsg3.tools[ri].name === data.tool && lastMsg3.tools[ri].running) {
                lastMsg3.tools[ri].running = false;
                // Collapse once finished. Only the step still running stays
                // open: a long task leaves dozens of these behind, and with
                // every one expanded the conversation became a scroll through
                // tool output to find what anyone actually said.
                lastMsg3.tools[ri].expanded = false;
                lastMsg3.tools[ri].result = data.result || '';
                lastMsg3.tools[ri].is_error = !!data.is_error;
                // Extract image URLs from image_generate or browser_screenshot results
                if ((data.tool === 'image_generate' || data.tool === 'browser_screenshot') && !data.is_error) {
                  try {
                    var parsed = JSON.parse(data.result);
                    if (parsed.image_urls && parsed.image_urls.length) {
                      lastMsg3.tools[ri]._imageUrls = parsed.image_urls;
                    }
                  } catch(e) { /* not JSON */ }
                }
                // Extract audio file path from text_to_speech results
                if (data.tool === 'text_to_speech' && !data.is_error) {
                  try {
                    var ttsResult = JSON.parse(data.result);
                    if (ttsResult.saved_to) {
                      lastMsg3.tools[ri]._audioFile = ttsResult.saved_to;
                      lastMsg3.tools[ri]._audioDuration = ttsResult.duration_estimate_ms;
                    }
                  } catch(e) { /* not JSON */ }
                }
                break;
              }
            }
            this.messages.splice(trIdx, 1, lastMsg3);
          }
          this.scrollToBottom();
          break;

        case 'response':
          this._clearTypingTimeout();
          this._stopBusyWatch();
          // Update context pressure from response
          if (data.context_pressure) {
            this.contextPressure = data.context_pressure;
          }
          // Collect streamed text before removing streaming messages
          var streamedText = '';
          var streamedTools = [];
          this.messages.forEach(function(m) {
            if (m.streaming && !m.thinking && m.role === 'agent') {
              streamedText += m.text || '';
              streamedTools = streamedTools.concat(m.tools || []);
            }
          });
          streamedTools.forEach(function(t) {
            t.running = false;
            t.expanded = false;
            // Text-detected tool calls (model leaked as text) — mark as not executed
            if (t.id && t.id.indexOf('-txt-') !== -1 && !t.result) {
              t.result = 'Model attempted this call as text (not executed via tool system)';
              t.is_error = true;
            }
          });
          this.messages = this.messages.filter(function(m) { return !m.thinking && !m.streaming; });
          var meta = (data.input_tokens || 0) + ' in / ' + (data.output_tokens || 0) + ' out';
          if (data.cost_usd != null) meta += ' | $' + data.cost_usd.toFixed(4);
          if (data.iterations) meta += ' | ' + data.iterations + ' iter';
          if (data.fallback_model) meta += ' | fallback: ' + data.fallback_model;
          // Use server response if non-empty, otherwise preserve accumulated streamed text
          var finalText = (data.content && data.content.trim()) ? data.content : streamedText;
          // Strip raw function-call JSON that some models leak as text
          finalText = this.sanitizeToolText(finalText);
          // If text is empty but tools ran, show a summary
          if (!finalText.trim() && streamedTools.length) {
            finalText = '';
          }
          this._pushAgentReply(finalText, meta, streamedTools, Date.now());
          this.sending = false;
          this.tokenCount = 0;
          this.scrollToBottom();
          var self3 = this;
          this.$nextTick(function() {
            var el = document.getElementById('msg-input'); if (el) el.focus();
            self3._processQueue();
          });
          break;

        case 'silent_complete':
          // Agent intentionally chose not to reply (NO_REPLY)
          this._clearTypingTimeout();
          this._stopBusyWatch();
          this.messages = this.messages.filter(function(m) { return !m.thinking && !m.streaming; });
          this.sending = false;
          this.tokenCount = 0;
          // No message bubble added — the agent was silent
          var selfSilent = this;
          this.$nextTick(function() { selfSilent._processQueue(); });
          break;

        case 'error':
          this._clearTypingTimeout();
          this._stopBusyWatch();
          this.messages = this.messages.filter(function(m) { return !m.thinking && !m.streaming; });
          this.messages.push({ id: ++msgId, role: 'system', text: 'Error: ' + data.content, meta: '', tools: [], ts: Date.now() });
          this.sending = false;
          this.tokenCount = 0;
          this.scrollToBottom();
          var self2 = this;
          this.$nextTick(function() {
            var el = document.getElementById('msg-input'); if (el) el.focus();
            self2._processQueue();
          });
          break;

        case 'agents_updated':
          // Refetch through the store rather than assigning the pushed list
          // straight in: the store is what puts the agent you talk to first,
          // and overwriting it here undid that on every agent change.
          if (data.agents) {
            Alpine.store('app').refreshAgents();
          }
          break;

        case 'command_result':
          // Update context pressure if included in command result
          if (data.context_pressure) {
            this.contextPressure = data.context_pressure;
          }
          this.messages.push({ id: ++msgId, role: 'system', text: data.message || 'Command executed.', meta: '', tools: [] });
          this.scrollToBottom();
          break;

        case 'canvas':
          // Agent presented an interactive canvas — render it in an iframe sandbox
          var canvasHtml = '<div class="canvas-panel" style="border:1px solid var(--border);border-radius:8px;margin:8px 0;overflow:hidden;">';
          canvasHtml += '<div style="padding:6px 12px;background:var(--surface);border-bottom:1px solid var(--border);font-size:0.85em;display:flex;justify-content:space-between;align-items:center;">';
          canvasHtml += '<span>' + (data.title || 'Canvas') + '</span>';
          canvasHtml += '<span style="opacity:0.5;font-size:0.8em;">' + (data.canvas_id || '').substring(0, 8) + '</span></div>';
          canvasHtml += '<iframe sandbox="allow-scripts" srcdoc="' + (data.html || '').replace(/"/g, '&quot;') + '" ';
          canvasHtml += 'style="width:100%;min-height:300px;border:none;background:#fff;" loading="lazy"></iframe></div>';
          this.messages.push({ id: ++msgId, role: 'agent', text: canvasHtml, meta: 'canvas', isHtml: true, tools: [] });
          this.scrollToBottom();
          break;

        case 'pong': break;
      }
    },

    // Format timestamp for display
    formatTime: function(ts) {
      if (!ts) return '';
      var d = new Date(ts);
      var h = d.getHours();
      var m = d.getMinutes();
      var ampm = h >= 12 ? 'PM' : 'AM';
      h = h % 12 || 12;
      return h + ':' + (m < 10 ? '0' : '') + m + ' ' + ampm;
    },

    // Copy message text to clipboard
    copyMessage: function(msg) {
      var text = msg.text || '';
      navigator.clipboard.writeText(text).then(function() {
        msg._copied = true;
        setTimeout(function() { msg._copied = false; }, 2000);
      }).catch(function() {});
    },

    _agentName: function(agentId) {
      var a = (Alpine.store('app').agents || []).filter(function(x) { return x.id === agentId; })[0];
      return a ? a.name : 'the agent';
    },

    // Leave the chat on screen without stopping anything in it.
    //
    // Its run keeps going on the server and is saved there when it finishes.
    // Messages queued behind it are handed to the server now, addressed to
    // their conversation: the server keeps them in order behind the run, so
    // they are answered while the user works in another chat.
    _parkCurrentChat: function() {
      var agent = this.currentAgent;
      this._stopBusyWatch();
      this._clearTypingTimeout();
      var queue = this.messageQueue.slice();
      var wasSending = this.sending;
      this.messageQueue = [];
      this.sending = false;
      if (!agent || (!wasSending && !queue.length)) return;
      // Everything typed here that the server has not answered yet, so it is
      // still on screen when the user comes back before its turn is saved.
      var unanswered = this.messages
        .filter(function(m) { return m.role === 'user' && m.pending; })
        .map(function(m) { return m.text; });
      var key = this._parkKey(agent.id, this.sessionId);
      var parked = this._parked[key] || { texts: [], sending: 0 };
      parked.texts = parked.texts.concat(unanswered);
      this._parked[key] = parked;
      if (queue.length) this._deliverParked(agent.id, this.sessionId, queue);
    },

    // Parked state belongs to one conversation: an agent can be working in
    // several, and one left mid-reply must not show up in another.
    _parkKey: function(agentId, sessionId) {
      return agentId + '|' + (sessionId || '');
    },

    // Moving to another conversation of the same agent: what was queued for
    // the one on screen is delivered to it, not to the one being opened, and
    // it is parked like a chat left for another agent.
    //
    // The agent's socket is dropped too, before the switch is even asked for:
    // frames of the run left behind would otherwise be drawn into the next
    // conversation. The run carries on and is saved; whoever opens the next
    // conversation connects again.
    _leaveConversation: function() {
      if (!this.currentAgent) return;
      this._parkCurrentChat();
      FreecoAPI.wsDisconnect();
      this._wsAgent = null;
    },

    _deliverParked: async function(agentId, sessionId, queue) {
      var parked = this._parked[this._parkKey(agentId, sessionId)];
      if (parked) parked.sending += queue.length;
      for (var i = 0; i < queue.length; i++) {
        var q = queue[i];
        var body = { message: q.text };
        if (q.files && q.files.length) body.attachments = q.files;
        if (sessionId) body.session_id = sessionId;
        try {
          await FreecoAPI.post('/api/agents/' + agentId + '/message', body);
        } catch (e) {
          FreecoToast.error('A queued message to ' + this._agentName(agentId) + ' failed: ' + e.message);
        }
        if (parked) parked.sending -= 1;
      }
    },

    // Messages of a parked chat that the server has not answered yet: not in
    // its history until their turn is saved, so drawn from here meanwhile.
    _showParkedMessages: function(parked) {
      var self = this;
      parked.texts.forEach(function(text) {
        self.messages.push({ id: ++msgId, role: 'user', text: text, meta: '', tools: [], images: [], ts: Date.now(), parked: true });
      });
    },

    // Show that the agent is working here, and reload once it has finished.
    //
    // Watched by conversation, not by agent: another window switching the
    // agent to a different conversation must not end this watch or swap what
    // this chat shows and where its queue goes.
    _watchBusy: function(agentId, sessionId) {
      this._stopBusyWatch();
      this.sending = true;
      if (!this.messages.some(function(m) { return m.thinking; })) {
        this.messages.push({ id: ++msgId, role: 'agent', text: 'Still working on this...', meta: '', thinking: true, tools: [], ts: Date.now() });
      }
      var self = this;
      var key = this._parkKey(agentId, sessionId);
      var here = function() {
        return self.currentAgent && self.currentAgent.id === agentId && self.sessionId === sessionId;
      };
      this._busyPoll = setInterval(async function() {
        if (!here()) { self._stopBusyWatch(); return; }
        var parked = self._parked[key];
        if (parked && parked.sending > 0) return;
        try {
          var url = '/api/agents/' + encodeURIComponent(agentId) + '/session';
          if (sessionId) url += '?session_id=' + encodeURIComponent(sessionId);
          var data = await FreecoAPI.get(url);
          if (!data || data.busy) return;
        } catch (e) { return; }
        if (!here() || !self._busyPoll) return;
        self._stopBusyWatch();
        delete self._parked[key];
        // Messages typed meanwhile stay: they are pending and still queued.
        self.messages = self.messages.filter(function(m) { return !m.thinking && !m.parked; });
        self.sending = false;
        await self.loadHistory(agentId, sessionId);
        self._processQueue();
      }, 3000);
    },

    _stopBusyWatch: function() {
      if (this._busyPoll) { clearInterval(this._busyPoll); this._busyPoll = null; }
    },

    // Process queued messages after current response completes
    _processQueue: function() {
      if (!this.messageQueue.length || this.sending) return;
      var next = this.messageQueue.shift();
      this._sendPayload(next.text, next.files, next.images);
    },

    async sendMessage() {
      if (!this.currentAgent || (!this.inputText.trim() && !this.attachments.length)) return;
      var text = this.inputText.trim();

      // Handle slash commands
      if (text.startsWith('/') && !this.attachments.length) {
        var cmd = text.split(' ')[0].toLowerCase();
        var cmdArgs = text.substring(cmd.length).trim();
        var matched = this.slashCommands.find(function(c) { return c.cmd === cmd; });
        if (matched) {
          this.executeSlashCommand(matched.cmd, cmdArgs);
          return;
        }
      }

      // A key in the message goes to the vault, not to the model. Null means
      // the user went back to edit: the text stays in the box.
      // `_guarding` keeps a second Enter during the check from sending twice.
      if (this._guarding) return;
      this._guarding = true;
      var guarded;
      try { guarded = await window.FreecoSecretGuard.check(text); } finally { this._guarding = false; }
      if (guarded === null) return;
      text = guarded;

      // No routing here: the main chat sends to the conversation on screen.
      // Deciding where a message belongs is the popup's job (owner's
      // decision, 2026-10-04); asking a router here made every send wait.

      this.inputText = '';

      // Reset textarea height to single line
      var ta = document.getElementById('msg-input');
      if (ta) ta.style.height = '';

      // Upload attachments first if any
      var fileRefs = [];
      var uploadedFiles = [];
      if (this.attachments.length) {
        for (var i = 0; i < this.attachments.length; i++) {
          var att = this.attachments[i];
          att.uploading = true;
          try {
            var uploadRes = await FreecoAPI.upload(this.currentAgent.id, att.file);
            fileRefs.push('[File: ' + att.file.name + ']');
            uploadedFiles.push({ file_id: uploadRes.file_id, filename: uploadRes.filename, content_type: uploadRes.content_type });
          } catch(e) {
            FreecoToast.error('Failed to upload ' + att.file.name);
            fileRefs.push('[File: ' + att.file.name + ' (upload failed)]');
          }
          att.uploading = false;
        }
        // Clean up previews
        for (var j = 0; j < this.attachments.length; j++) {
          if (this.attachments[j].preview) URL.revokeObjectURL(this.attachments[j].preview);
        }
        this.attachments = [];
      }

      // Build final message text
      var finalText = text;
      if (fileRefs.length) {
        finalText = (text ? text + '\n' : '') + fileRefs.join('\n');
      }

      // Collect image references for inline rendering
      var msgImages = uploadedFiles.filter(function(f) { return f.content_type && f.content_type.startsWith('image/'); });

      // Always show user message immediately, and mark it pending until the
      // server's own history contains it, so a reload cannot drop it.
      this.messages.push({ id: ++msgId, role: 'user', text: finalText, meta: '', tools: [], images: msgImages, ts: Date.now(), pending: true });
      this.scrollToBottom(true);
      localStorage.setItem('of-first-msg', 'true');

      // If already streaming, queue this message
      if (this.sending) {
        this.messageQueue.push({ text: finalText, files: uploadedFiles, images: msgImages });
        return;
      }

      this._sendPayload(finalText, uploadedFiles, msgImages);
    },

    async _sendPayload(finalText, uploadedFiles, msgImages) {
      this.sending = true;
      var agentId = this.currentAgent.id;

      // Try WebSocket first
      var wsPayload = { type: 'message', content: finalText };
      if (uploadedFiles && uploadedFiles.length) wsPayload.attachments = uploadedFiles;
      if (this.sessionId) wsPayload.session_id = this.sessionId;
      if (FreecoAPI.wsSend(wsPayload)) {
        this.messages.push({ id: ++msgId, role: 'agent', text: '', meta: '', thinking: true, streaming: true, tools: [], ts: Date.now() });
        this.scrollToBottom();
        return;
      }

      // HTTP fallback
      var t = typeof window.t === 'function' ? window.t : function(s) { return s; };
      if (!FreecoAPI.isWsConnected()) {
        FreecoToast.info(t('chat.using_http_mode'));
      }
      this.messages.push({ id: ++msgId, role: 'agent', text: '', meta: '', thinking: true, tools: [], ts: Date.now() });
      this.scrollToBottom();

      var sentSession = this.sessionId;
      var self0 = this;
      // The user moved to another chat, or another conversation of this
      // agent, while this ran. The reply is saved with its conversation and
      // shows when that is opened again; drawing it here would put it into a
      // chat it does not belong to.
      var left = function() {
        return !self0.currentAgent || self0.currentAgent.id !== agentId || self0.sessionId !== sentSession;
      };
      try {
        var httpBody = { message: finalText };
        if (uploadedFiles && uploadedFiles.length) httpBody.attachments = uploadedFiles;
        if (sentSession) httpBody.session_id = sentSession;
        var res = await FreecoAPI.post('/api/agents/' + agentId + '/message', httpBody);
        if (left()) return;
        this.messages = this.messages.filter(function(m) { return !m.thinking; });
        var httpMeta = (res.input_tokens || 0) + ' in / ' + (res.output_tokens || 0) + ' out';
        if (res.cost_usd != null) httpMeta += ' | $' + res.cost_usd.toFixed(4);
        if (res.iterations) httpMeta += ' | ' + res.iterations + ' iter';
        this._pushAgentReply(res.response, httpMeta, [], Date.now());
      } catch(e) {
        if (left()) {
          FreecoToast.error('A message did not reach ' + this._agentName(agentId) + ': ' + e.message);
          return;
        }
        this.messages = this.messages.filter(function(m) { return !m.thinking; });
        this.messages.push({ id: ++msgId, role: 'system', text: 'Error: ' + e.message, meta: '', tools: [], ts: Date.now() });
      }
      this.sending = false;
      this.scrollToBottom();
      // Process next queued message
      var self = this;
      this.$nextTick(function() {
        var el = document.getElementById('msg-input'); if (el) el.focus();
        self._processQueue();
      });
    },

    // Stop the current agent run
    stopAgent: function() {
      if (!this.currentAgent) return;
      var self = this;
      FreecoAPI.post('/api/agents/' + this.currentAgent.id + '/stop', {}).then(function(res) {
        self.messages.push({ id: ++msgId, role: 'system', text: res.message || 'Run cancelled', meta: '', tools: [], ts: Date.now() });
        self.sending = false;
        self.scrollToBottom();
        self.$nextTick(function() { self._processQueue(); });
      }).catch(function(e) { FreecoToast.error('Stop failed: ' + e.message); });
    },

    killAgent() {
      if (!this.currentAgent) return;
      var self = this;
      var t = typeof window.t === 'function' ? window.t : function(s) { return s; };
      var name = this.currentAgent.name;
      FreecoToast.confirm(t('chat.stop_agent_title'), t('chat.stop_agent_confirm') + ' "' + name + '"?', async function() {
        try {
          await FreecoAPI.del('/api/agents/' + self.currentAgent.id);
          FreecoAPI.wsDisconnect();
          self._wsAgent = null;
          self.currentAgent = null;
          self.messages = [];
          try { localStorage.removeItem('of-active-agent'); } catch(e) { /* ignore */ }
          FreecoToast.success(t('chat.agent_stopped') + ' "' + name + '"');
          Alpine.store('app').refreshAgents();
        } catch(e) {
          FreecoToast.error(t('chat.stop_agent_failed') + ': ' + e.message);
        }
      });
    },

    // Permanently uninstall the agent: kill + remove ~/.freeco-ai/agents/<name>/
    // Issue #1163.
    uninstallAgent: function() {
      if (!this.currentAgent) return;
      var self = this;
      var name = this.currentAgent.name;
      var agentId = this.currentAgent.id;
      FreecoToast.confirm(
        'Uninstall Agent',
        'Uninstall agent "' + name + '"? This stops the agent AND deletes its files from your workspace. This cannot be undone.',
        async function() {
          try {
            var res = await FreecoAPI.del('/api/agents/' + agentId + '/uninstall');
            FreecoAPI.wsDisconnect();
            self._wsAgent = null;
            self.currentAgent = null;
            self.messages = [];
            try { localStorage.removeItem('of-active-agent'); } catch(e) { /* ignore */ }
            var msg = 'Agent "' + name + '" uninstalled';
            if (res && res.dir_removed === false) {
              msg += ' (no on-disk files found)';
            }
            FreecoToast.success(msg);
            Alpine.store('app').refreshAgents();
          } catch(e) {
            FreecoToast.error('Failed to uninstall agent: ' + e.message);
          }
        }
      );
    },

    _latexTimer: null,
    // Follow the conversation only while the reader is already at the end.
    //
    // Every streamed token called this, and it moved the view to the bottom
    // unconditionally -- so anyone who scrolled up to reread an earlier message
    // while the agent was writing was dragged back down on the next token and
    // could not read anything until the reply finished. Now the view follows
    // only when it was already at the bottom; otherwise it stays put and
    // `hasUnseenBelow` offers a way back. `force` is for the user's own actions
    // -- sending, opening a chat -- where they expect to see the end.
    _stickToBottom: true,
    hasUnseenBelow: false,
    _nearBottom(el) {
      return el.scrollHeight - el.scrollTop - el.clientHeight < 120;
    },
    onMessagesScroll() {
      var el = document.getElementById('messages');
      if (!el) return;
      this._stickToBottom = this._nearBottom(el);
      if (this._stickToBottom) this.hasUnseenBelow = false;
    },
    jumpToBottom() {
      this._stickToBottom = true;
      this.hasUnseenBelow = false;
      this.scrollToBottom(true);
    },
    scrollToBottom(force) {
      var self = this;
      var el = document.getElementById('messages');
      if (!el) return;
      if (!force && !self._stickToBottom) {
        self.hasUnseenBelow = true;
        return;
      }
      if (force) { self._stickToBottom = true; self.hasUnseenBelow = false; }
      self.$nextTick(function() {
        el.scrollTop = el.scrollHeight;
        // Debounce LaTeX rendering to avoid running on every streaming token
        if (self._latexTimer) clearTimeout(self._latexTimer);
        self._latexTimer = setTimeout(function() { renderLatex(el); }, 150);
      });
    },

    addFiles(files) {
      var self = this;
      var allowed = ['image/png', 'image/jpeg', 'image/gif', 'image/webp', 'text/plain', 'application/pdf',
                      'text/markdown', 'application/json', 'text/csv'];
      var allowedExts = ['.txt', '.pdf', '.md', '.json', '.csv'];
      for (var i = 0; i < files.length; i++) {
        var file = files[i];
        if (file.size > 10 * 1024 * 1024) {
          FreecoToast.warn('File "' + file.name + '" exceeds 10MB limit');
          continue;
        }
        var typeOk = allowed.indexOf(file.type) !== -1;
        if (!typeOk) {
          var ext = file.name.lastIndexOf('.') !== -1 ? file.name.substring(file.name.lastIndexOf('.')).toLowerCase() : '';
          typeOk = allowedExts.indexOf(ext) !== -1 || file.type.startsWith('image/');
        }
        if (!typeOk) {
          FreecoToast.warn('File type not supported: ' + file.name);
          continue;
        }
        var preview = null;
        if (file.type.startsWith('image/')) {
          preview = URL.createObjectURL(file);
        }
        self.attachments.push({ file: file, preview: preview, uploading: false });
      }
    },

    removeAttachment(idx) {
      var att = this.attachments[idx];
      if (att && att.preview) URL.revokeObjectURL(att.preview);
      this.attachments.splice(idx, 1);
    },

    handleDrop(e) {
      e.preventDefault();
      if (e.dataTransfer && e.dataTransfer.files && e.dataTransfer.files.length) {
        this.addFiles(e.dataTransfer.files);
      }
    },

    isGrouped(idx) {
      if (idx === 0) return false;
      var prev = this.messages[idx - 1];
      var curr = this.messages[idx];
      return prev && curr && prev.role === curr.role && !curr.thinking && !prev.thinking;
    },

    // Strip raw function-call text that some models (Llama, Groq, etc.) leak into output.
    // These models don't use proper tool_use blocks — they output function calls as plain text.
    sanitizeToolText: function(text) {
      if (!text) return text;
      // Pattern: tool_name</function={"key":"value"} or tool_name</function,{...}
      text = text.replace(/\s*\w+<\/function[=,]?\s*\{[\s\S]*$/gm, '');
      // Pattern: <function=tool_name>{...}</function>
      text = text.replace(/<function=\w+>[\s\S]*?<\/function>/g, '');
      // Pattern: tool_name{"type":"function",...}
      text = text.replace(/\s*\w+\{"type"\s*:\s*"function"[\s\S]*$/gm, '');
      // Pattern: lone </function...> tags
      text = text.replace(/<\/function[^>]*>/g, '');
      // Pattern: <|python_tag|> or similar special tokens
      text = text.replace(/<\|[\w_]+\|>/g, '');
      return text.trim();
    },

    formatToolJson: function(text) {
      if (!text) return '';
      if (typeof text === 'object') {
        return JSON.stringify(text, null, 2);
      }
      try { return JSON.stringify(JSON.parse(text), null, 2); }
      catch(e) { return text; }
    },

    // Voice: start recording
    startRecording: async function() {
      if (this.recording) return;
      try {
        var stream = await navigator.mediaDevices.getUserMedia({ audio: true });
        var mimeType = MediaRecorder.isTypeSupported('audio/webm;codecs=opus') ? 'audio/webm;codecs=opus' :
                       MediaRecorder.isTypeSupported('audio/webm') ? 'audio/webm' : 'audio/ogg';
        this._audioChunks = [];
        this._mediaRecorder = new MediaRecorder(stream, { mimeType: mimeType });
        var self = this;
        this._mediaRecorder.ondataavailable = function(e) {
          if (e.data.size > 0) self._audioChunks.push(e.data);
        };
        this._mediaRecorder.onstop = function() {
          stream.getTracks().forEach(function(t) { t.stop(); });
          self._handleRecordingComplete();
        };
        this._mediaRecorder.start(250);
        this.recording = true;
        this.recordingTime = 0;
        this._recordingTimer = setInterval(function() { self.recordingTime++; }, 1000);
      } catch(e) {
        if (typeof FreecoToast !== 'undefined') FreecoToast.error('Microphone access denied');
      }
    },

    // Voice: stop recording
    stopRecording: function() {
      if (!this.recording || !this._mediaRecorder) return;
      this._mediaRecorder.stop();
      this.recording = false;
      if (this._recordingTimer) { clearInterval(this._recordingTimer); this._recordingTimer = null; }
    },

    // Voice: handle completed recording — upload and transcribe
    _handleRecordingComplete: async function() {
      if (!this._audioChunks.length || !this.currentAgent) return;
      var blob = new Blob(this._audioChunks, { type: this._audioChunks[0].type || 'audio/webm' });
      this._audioChunks = [];
      if (blob.size < 100) return; // too small

      // Show a temporary "Transcribing..." message
      this.messages.push({ id: ++msgId, role: 'system', text: 'Transcribing audio...', thinking: true, ts: Date.now(), tools: [] });
      this.scrollToBottom();

      try {
        // Upload audio file
        var ext = blob.type.includes('webm') ? 'webm' : blob.type.includes('ogg') ? 'ogg' : 'mp3';
        var file = new File([blob], 'voice_' + Date.now() + '.' + ext, { type: blob.type });
        var upload = await FreecoAPI.upload(this.currentAgent.id, file);

        // Remove the "Transcribing..." message
        this.messages = this.messages.filter(function(m) { return !m.thinking || m.role !== 'system'; });

        // Use server-side transcription when available. If speech-to-text is not
        // configured the server now tells us why — surface that instead of
        // silently sending an opaque "[Voice message]" the agent can't read.
        var said = (upload.transcription && upload.transcription.trim()) ? upload.transcription.trim() : '';
        if (!said) {
          this.messages.push({
            id: ++msgId, role: 'system', ts: Date.now(), tools: [],
            text: upload.transcription_error
              ? ('Voice: ' + upload.transcription_error)
              : 'Voice: could not transcribe that audio. Check your speech-to-text setup in Settings.'
          });
          this.scrollToBottom();
          return;
        }
        this._sendPayload(said, [upload], []);
      } catch(e) {
        this.messages = this.messages.filter(function(m) { return !m.thinking || m.role !== 'system'; });
        if (typeof FreecoToast !== 'undefined') FreecoToast.error('Failed to upload audio: ' + (e.message || 'unknown error'));
      }
    },

    // Voice: format recording time as MM:SS
    formatRecordingTime: function() {
      var m = Math.floor(this.recordingTime / 60);
      var s = this.recordingTime % 60;
      return (m < 10 ? '0' : '') + m + ':' + (s < 10 ? '0' : '') + s;
    },

    // Search: toggle open/close
    toggleSearch: function() {
      this.searchOpen = !this.searchOpen;
      if (this.searchOpen) {
        var self = this;
        this.$nextTick(function() {
          var el = document.getElementById('chat-search-input');
          if (el) el.focus();
        });
      } else {
        this.searchQuery = '';
      }
    },

    // Search: filter messages by query
    get filteredMessages() {
      if (!this.searchQuery.trim()) return this.messages;
      var q = this.searchQuery.toLowerCase();
      return this.messages.filter(function(m) {
        return (m.text && m.text.toLowerCase().indexOf(q) !== -1) ||
               (m.tools && m.tools.some(function(t) { return t.name.toLowerCase().indexOf(q) !== -1; }));
      });
    },

    // Search: highlight matched text in a string
    highlightSearch: function(html) {
      if (!this.searchQuery.trim() || !html) return html;
      var q = this.searchQuery.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
      var regex = new RegExp('(' + q + ')', 'gi');
      return html.replace(regex, '<mark style="background:var(--warning);color:var(--bg);border-radius:2px;padding:0 2px">$1</mark>');
    },

    renderMarkdown: renderMarkdown,
    escapeHtml: escapeHtml
  };

  // Permission requests are answered here too, not only in the floating
  // assistant, mixed in from one place so the two chat surfaces cannot drift
  // apart again.
  //
  // Copied onto the page object rather than the page object being copied onto
  // the extras: `Object.assign` reads a getter and writes its *value*, so
  // spreading this object would evaluate all ten of its computed properties
  // once at creation and freeze them. `filteredSwitcherModels` and
  // `filteredModelPicker` became empty arrays that never recomputed -- the
  // model list read "No models found" and no conversation would open.
  //
  // Existing keys are never overwritten, so the page always wins over a shared
  // default.
  var extras = window.FreecoInlineApprovals ? window.FreecoInlineApprovals() : {};
  Object.keys(extras).forEach(function (k) {
    if (!(k in page)) page[k] = extras[k];
  });
  return page;
}
