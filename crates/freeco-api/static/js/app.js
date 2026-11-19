
/* Typography set. Applied before first paint from localStorage so the page
   never flashes the default face and then re-renders in the chosen one. */
(function () {
  try {
    // Clear any previously stored set. The serif experiment was reverted, and
    // a browser that had already saved 'editorial' would otherwise keep
    // applying it after the default changed back.
    var t = localStorage.getItem('freeco-type');
    if (t === 'editorial') { localStorage.removeItem('freeco-type'); t = null; }
    if (t) document.documentElement.setAttribute('data-type', t);
  } catch (e) { /* private mode: fall back to the default set */ }
})();

window.setTypography = function (name) {
  if (name) {
    document.documentElement.setAttribute('data-type', name);
  } else {
    document.documentElement.removeAttribute('data-type');
  }
  try { localStorage.setItem('freeco-type', name || ''); } catch (e) {}
};
// FreEco.ai App — Alpine.js init, hash router, global store
'use strict';

// Marked.js configuration
if (typeof marked !== 'undefined') {
  marked.setOptions({
    breaks: true,
    gfm: true,
    highlight: function(code, lang) {
      if (typeof hljs !== 'undefined' && lang && hljs.getLanguage(lang)) {
        try { return hljs.highlight(code, { language: lang }).value; } catch(e) {}
      }
      return code;
    }
  });
}

function escapeHtml(text) {
  var div = document.createElement('div');
  div.textContent = text || '';
  return div.innerHTML.replace(/\n/g, '<br>');
}

function renderMarkdown(text) {
  if (!text) return '';
  if (typeof marked !== 'undefined') {
    // Protect LaTeX blocks from marked.js mangling (underscores, backslashes, etc.)
    var latexBlocks = [];
    var protected_ = text;
    // Protect display math $$...$$ first (greedy across lines)
    protected_ = protected_.replace(/\$\$([\s\S]+?)\$\$/g, function(match) {
      var idx = latexBlocks.length;
      latexBlocks.push(match);
      return '\x00LATEX' + idx + '\x00';
    });
    // Protect inline math $...$ (single line, not empty, not starting/ending with space)
    protected_ = protected_.replace(/\$([^\s$](?:[^$]*[^\s$])?)\$/g, function(match) {
      var idx = latexBlocks.length;
      latexBlocks.push(match);
      return '\x00LATEX' + idx + '\x00';
    });
    // Protect \[...\] display math
    protected_ = protected_.replace(/\\\[([\s\S]+?)\\\]/g, function(match) {
      var idx = latexBlocks.length;
      latexBlocks.push(match);
      return '\x00LATEX' + idx + '\x00';
    });
    // Protect \(...\) inline math
    protected_ = protected_.replace(/\\\(([\s\S]+?)\\\)/g, function(match) {
      var idx = latexBlocks.length;
      latexBlocks.push(match);
      return '\x00LATEX' + idx + '\x00';
    });

    var html = marked.parse(protected_);
    // Restore LaTeX blocks
    for (var i = 0; i < latexBlocks.length; i++) {
      html = html.replace('\x00LATEX' + i + '\x00', latexBlocks[i]);
    }
    // Add copy buttons to code blocks
    html = html.replace(/<pre><code/g, '<pre><button class="copy-btn" onclick="copyCode(this)">Copy</button><code');
    // Open external links in new tab
    html = html.replace(/<a\s+href="(https?:\/\/[^"]*)"(?![^>]*target=)([^>]*)>/gi, '<a href="$1" target="_blank" rel="noopener"$2>');
    return html;
  }
  return escapeHtml(text);
}

function copyCode(btn) {
  var code = btn.nextElementSibling;
  if (code) {
    navigator.clipboard.writeText(code.textContent).then(function() {
      btn.textContent = 'Copied!';
      btn.classList.add('copied');
      setTimeout(function() { btn.textContent = 'Copy'; btn.classList.remove('copied'); }, 1500);
    });
  }
}

// Tool category icon SVGs — returns inline SVG for each tool category
function toolIcon(toolName) {
  if (!toolName) return '';
  var n = toolName.toLowerCase();
  var s = 'width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"';
  // File/directory operations
  if (n.indexOf('file_') === 0 || n.indexOf('directory_') === 0)
    return '<svg ' + s + '><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6"/><path d="M16 13H8"/><path d="M16 17H8"/></svg>';
  // Web/fetch
  if (n.indexOf('web_') === 0 || n.indexOf('link_') === 0)
    return '<svg ' + s + '><circle cx="12" cy="12" r="10"/><path d="M2 12h20"/><path d="M12 2a15 15 0 0 1 4 10 15 15 0 0 1-4 10 15 15 0 0 1-4-10 15 15 0 0 1 4-10z"/></svg>';
  // Shell/exec
  if (n.indexOf('shell') === 0 || n.indexOf('exec_') === 0)
    return '<svg ' + s + '><polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/></svg>';
  // Agent operations
  if (n.indexOf('agent_') === 0)
    return '<svg ' + s + '><path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M23 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/></svg>';
  // Memory/knowledge
  if (n.indexOf('memory_') === 0 || n.indexOf('knowledge_') === 0)
    return '<svg ' + s + '><path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z"/><path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"/></svg>';
  // Cron/schedule
  if (n.indexOf('cron_') === 0 || n.indexOf('schedule_') === 0)
    return '<svg ' + s + '><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg>';
  // Browser/playwright
  if (n.indexOf('browser_') === 0 || n.indexOf('playwright_') === 0)
    return '<svg ' + s + '><rect x="2" y="3" width="20" height="14" rx="2"/><path d="M8 21h8"/><path d="M12 17v4"/></svg>';
  // Container/docker
  if (n.indexOf('container_') === 0 || n.indexOf('docker_') === 0)
    return '<svg ' + s + '><path d="M22 12H2"/><path d="M5.45 5.11L2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/></svg>';
  // Image/media
  if (n.indexOf('image_') === 0 || n.indexOf('tts_') === 0)
    return '<svg ' + s + '><rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8.5" cy="8.5" r="1.5"/><polyline points="21 15 16 10 5 21"/></svg>';
  // Hand tools
  if (n.indexOf('hand_') === 0)
    return '<svg ' + s + '><path d="M18 11V6a2 2 0 0 0-2-2 2 2 0 0 0-2 2"/><path d="M14 10V4a2 2 0 0 0-2-2 2 2 0 0 0-2 2v6"/><path d="M10 10.5V6a2 2 0 0 0-2-2 2 2 0 0 0-2 2v8"/><path d="M18 8a2 2 0 1 1 4 0v6a8 8 0 0 1-8 8h-2c-2.8 0-4.5-.9-5.7-2.4L3.4 16a2 2 0 0 1 3.2-2.4L8 15"/></svg>';
  // Task/collab
  if (n.indexOf('task_') === 0)
    return '<svg ' + s + '><path d="M9 11l3 3L22 4"/><path d="M21 12v7a2 2 0 01-2 2H5a2 2 0 01-2-2V5a2 2 0 012-2h11"/></svg>';
  // Default — wrench
  return '<svg ' + s + '><path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/></svg>';
}

// Alpine.js global store
document.addEventListener('alpine:init', function() {
  // Restore saved API key on load
  var savedKey = localStorage.getItem('freeco-api-key');
  if (savedKey) FreecoAPI.setAuthToken(savedKey);

  Alpine.store('app', {
    agents: [],
    connected: false,
    booting: true,
    wsConnected: false,
    connectionState: 'connected',
    lastError: '',
    version: '0.1.0',
    agentCount: 0,
    pendingApprovalCount: 0,
    lastPendingApprovalSignature: '',
    pendingAgent: null,
    // A conversation picked in the sidebar, opened once its agent's chat is.
    pendingSession: null,
    // The second sidebar (chats, projects, tasks). Shown unless hidden, and
    // a drawer of its own on a phone.
    sideNavHidden: localStorage.getItem('freeco-sidenav') === 'hidden',
    sideNavMobile: false,
    toggleSideNav() {
      this.sideNavHidden = !this.sideNavHidden;
      try { localStorage.setItem('freeco-sidenav', this.sideNavHidden ? 'hidden' : 'shown'); } catch (e) { /* private window */ }
    },
    focusMode: localStorage.getItem('freeco-focus') === 'true',
    showOnboarding: false,
    showAuthPrompt: false,
    showPasswordSetup: false,
    _setupSkipped: false,  // once true this session, the setup prompt never re-shows
    authMode: 'apikey',
    sessionUser: null,
    sessionRole: null,  // RBAC role of the logged-in dashboard user (owner|admin|user|kid|viewer)
    updateAvailable: false,
    updateLatest: '',
    frozen: false,
    freezeBusy: false,

    async refreshFreeze() {
      try {
        var result = await FreecoAPI.get('/api/system/freeze');
        this.frozen = !!result.frozen;
      } catch(e) { /* do not obscure the dashboard when the optional control is unavailable */ }
    },

    async toggleEmergencyFreeze() {
      if (this.freezeBusy) return;
      this.freezeBusy = true;
      try {
        if (this.frozen) {
          var released = await FreecoAPI.del('/api/system/freeze');
          this.frozen = !!released.frozen;
          FreecoToast.success('Emergency stop released. ' + (released.resumed || 0) + ' agent(s) resumed.');
        } else {
          var engaged = await FreecoAPI.post('/api/system/freeze', {});
          this.frozen = !!engaged.frozen;
          FreecoToast.warn('Emergency stop engaged. ' + (engaged.suspended || 0) + ' agent(s) paused.');
        }
      } catch(e) {
        FreecoToast.error('Could not change emergency stop: ' + e.message);
      }
      this.freezeBusy = false;
    },

    // Quiet daily update check (Settings → System has the manual button
    // and the on/off toggle; both share the freeco_* localStorage keys).
    async quietUpdateCheck() {
      if (localStorage.getItem('freeco_auto_update_check') === 'off') return;
      var last = Number(localStorage.getItem('freeco_update_last_check') || 0);
      if (Date.now() - last < 24 * 60 * 60 * 1000) return;
      try {
        var res = await fetch('https://api.github.com/repos/FreecoDAO/freeco-ai/releases/latest', { headers: { Accept: 'application/vnd.github+json' } });
        if (!res.ok) return;
        var rel = await res.json();
        var latest = String(rel.tag_name || '').replace(/^v/, '').split('.').map(Number);
        var current = String(this.version || '0.0.0').replace(/^v/, '').split('.').map(Number);
        for (var i = 0; i < Math.max(latest.length, current.length); i++) {
          var a = latest[i] || 0, b = current[i] || 0;
          if (a !== b) { this.updateAvailable = a > b; break; }
        }
        this.updateLatest = String(rel.tag_name || '').replace(/^v/, '');
        localStorage.setItem('freeco_update_last_check', String(Date.now()));
      } catch (e) { /* silent — never bother the user over a failed check */ }
    },

    toggleFocusMode() {
      this.focusMode = !this.focusMode;
      localStorage.setItem('freeco-focus', this.focusMode);
    },

    // The agent you talk to comes first, everywhere.
    //
    // Agents arrived in registry order, so the one that is the point of
    // contact - the CEO, concierge and executive assistant - sat wherever it
    // happened to land, and which agent to address was left to be guessed from
    // a list of names. The daemon already answers that question at
    // /api/assistant, by role rather than by name, so ask it once and keep the
    // answer first; the rest stay in the order they came, sorted by name.
    async refreshAgents() {
      try {
        var agents = await FreecoAPI.get('/api/agents');
        var list = Array.isArray(agents) ? agents : [];
        if (this._assistantId === undefined) {
          this._assistantId = null;
          try {
            var who = await FreecoAPI.get('/api/assistant');
            if (who && who.id) this._assistantId = who.id;
          } catch (e) { /* fall back to plain ordering */ }
        }
        var lead = this._assistantId;
        list.sort(function (a, b) {
          if (lead) {
            if (a.id === lead) return -1;
            if (b.id === lead) return 1;
          }
          return String(a.name || '').localeCompare(String(b.name || ''));
        });
        this.agents = list;
        this.agentCount = this.agents.length;
      } catch(e) { /* silent */ }
    },

    async refreshApprovals() {
      try {
        var data = await FreecoAPI.get('/api/approvals');
        var approvals = Array.isArray(data) ? data : (data.approvals || []);
        var pending = approvals.filter(function(a) { return a.status === 'pending'; });
        var signature = pending
          .map(function(a) { return a.id; })
          .sort()
          .join(',');
        // Alert only for requests not seen before.
        //
        // This compared the whole set, so approving one of two pending
        // requests changed the set and announced "an agent is waiting" again
        // for the one already on screen. And it was a toast only: silent, and
        // gone if the dashboard was not the window in front -- which is exactly
        // when an agent blocked on a shell approval sits unnoticed.
        var seen = this._seenApprovalIds || (this._seenApprovalIds = {});
        var firstLoad = !this._approvalsLoadedOnce;
        var fresh = pending.filter(function (a) { return !seen[a.id]; });
        pending.forEach(function (a) { seen[a.id] = true; });
        this._approvalsLoadedOnce = true;
        // Nothing announced on the first poll: those were already waiting
        // before this page opened, and the badge shows them.
        if (fresh.length && !firstLoad) {
          var first = fresh[0];
          var what = first.summary || first.description || first.command ||
                     first.tool_name || first.action || 'an action';
          var who = first.agent_name || first.agent || 'An agent';
          var text = who + ' is waiting for approval: ' + String(what).slice(0, 140) +
                     (fresh.length > 1 ? ' (+' + (fresh.length - 1) + ' more)' : '');
          if (typeof FreecoToast !== 'undefined') FreecoToast.warn(text);
          if (window.FreecoAlert) {
            window.FreecoAlert.chime();
            window.FreecoAlert.notify('FreEco.ai needs your approval', text, 'approvals');
          }
        }
        this.pendingApprovalCount = pending.length;
        this.lastPendingApprovalSignature = signature;
      } catch(e) { /* silent */ }
    },

    async checkStatus() {
      try {
        var s = await FreecoAPI.get('/api/status');
        this.connected = true;
        this.booting = false;
        this.lastError = '';
        this.version = s.version || '0.1.0';
        this.agentCount = s.agent_count || 0;
        this.quietUpdateCheck();
      } catch(e) {
        this.connected = false;
        this.lastError = e.message || 'Unknown error';
        console.warn('[FreEco.ai] Status check failed:', e.message);
      }
    },

    async checkOnboarding() {
      if (localStorage.getItem('freeco-onboarded')) return;
      try {
        var config = await FreecoAPI.get('/api/config');
        var apiKey = config && config.api_key;
        var noKey = !apiKey || apiKey === 'not set' || apiKey === '';
        if (noKey && this.agentCount === 0) {
          this.showOnboarding = true;
        }
      } catch(e) {
        // If config endpoint fails, still show onboarding if no agents
        if (this.agentCount === 0) this.showOnboarding = true;
      }
    },

    dismissOnboarding() {
      this.showOnboarding = false;
      localStorage.setItem('freeco-onboarded', 'true');
    },

    async checkAuth() {
      try {
        // First check if session-based auth is configured
        var authInfo = await FreecoAPI.get('/api/auth/check');
        // Server-persisted dismissal wins (survives the desktop's random-port
        // relaunch, which wipes origin-scoped localStorage). localStorage is
        // kept only as a same-session fast path.
        if (!this._setupSkipped && !authInfo.password_configured && !authInfo.setup_dismissed && !localStorage.getItem('freeco-password-setup-skipped')) {
          this.showPasswordSetup = true;
        }
        if (authInfo.mode === 'none') {
          // No session auth — fall back to API key detection
          this.authMode = 'apikey';
          this.sessionUser = null;
        } else if (authInfo.mode === 'session') {
          this.authMode = 'session';
          if (authInfo.authenticated) {
            this.sessionUser = authInfo.username;
            this.sessionRole = authInfo.role || 'owner';
            this.showAuthPrompt = false;
            return;
          }
          // Session auth enabled but not authenticated — show login prompt
          this.showAuthPrompt = true;
          return;
        }
      } catch(e) { /* ignore — fall through to API key check */ }

      // API key mode detection
      try {
        await FreecoAPI.get('/api/tools');
        this.showAuthPrompt = false;
      } catch(e) {
        if (e.message && (e.message.indexOf('Not authorized') >= 0 || e.message.indexOf('401') >= 0 || e.message.indexOf('Missing Authorization') >= 0 || e.message.indexOf('Unauthorized') >= 0)) {
          var saved = localStorage.getItem('freeco-api-key');
          if (saved) {
            FreecoAPI.setAuthToken('');
            localStorage.removeItem('freeco-api-key');
          }
          this.showAuthPrompt = true;
        }
      }
    },

    submitApiKey(key) {
      if (!key || !key.trim()) return;
      FreecoAPI.setAuthToken(key.trim());
      localStorage.setItem('freeco-api-key', key.trim());
      this.showAuthPrompt = false;
      this.refreshAgents();
    },

    async sessionLogin(username, password) {
      try {
        var result = await FreecoAPI.post('/api/auth/login', { username: username, password: password });
        if (result.status === 'ok') {
          this.sessionUser = result.username;
          this.sessionRole = result.role || 'owner';
          this.showAuthPrompt = false;
          this.refreshAgents();
        } else {
          FreecoToast.error(result.error || 'Login failed');
        }
      } catch(e) {
        FreecoToast.error(e.message || 'Login failed');
      }
    },

    async setInitialPassword(password, confirmation) {
      if (password !== confirmation) {
        FreecoToast.error('Passwords do not match');
        return;
      }
      try {
        var result = await FreecoAPI.post('/api/auth/set-password', { password: password });
        if (result.status === 'ok') {
          this.showPasswordSetup = false;
          localStorage.setItem('freeco-password-setup-skipped', 'true');
          FreecoAPI.post('/api/auth/dismiss-setup', {}).catch(function() {});
          FreecoToast.success('Password saved. Restart FreEco.ai to enable sign-in.');
        }
      } catch(e) {
        FreecoToast.error(e.message || 'Could not save password');
      }
    },

    skipPasswordSetup() {
      // In-memory guard first: even if the endpoint and localStorage both fail,
      // the prompt cannot re-appear this session.
      this._setupSkipped = true;
      this.showPasswordSetup = false;
      try { localStorage.setItem('freeco-password-setup-skipped', 'true'); } catch(e) { /* private mode */ }
      // Persist server-side so the prompt stays dismissed across restarts.
      FreecoAPI.post('/api/auth/dismiss-setup', {}).catch(function() {});
    },

    // Escape hatch on the login screen: turn dashboard auth off (loopback only)
    // for a user who set a password and now wants to use FreEco.ai without one,
    // or who forgot it. Requires a restart to fully apply.
    async disableAuth() {
      try {
        await FreecoAPI.post('/api/auth/disable', {});
        this.showAuthPrompt = false;
        this._setupSkipped = true;
        FreecoToast.success('Password turned off. Restart FreEco.ai to finish.');
      } catch(e) {
        FreecoToast.error(e.message || 'Could not turn off the password');
      }
    },

    async sessionLogout() {
      try {
        await FreecoAPI.post('/api/auth/logout');
      } catch(e) { /* ignore */ }
      this.sessionUser = null;
      this.showAuthPrompt = true;
    },

    clearApiKey() {
      FreecoAPI.setAuthToken('');
      localStorage.removeItem('freeco-api-key');
    }
  });
});

// Main app component
function app() {
  return {
    page: 'agents',
    themeMode: localStorage.getItem('freeco-theme-mode') || 'system',
    theme: (() => {
      var mode = localStorage.getItem('freeco-theme-mode') || 'system';
      if (mode === 'system') return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
      return mode;
    })(),
    // The main menu is an icon rail unless the user widened it: the second
    // sidebar beside it holds the chats, so both at full width would leave the
    // chat itself too narrow.
    sidebarCollapsed: localStorage.getItem('freeco-sidebar') !== 'expanded',
    mobileMenuOpen: false,
    connected: false,
    wsConnected: false,
    version: '0.1.0',
    agentCount: 0,

    get agents() { return Alpine.store('app').agents; },

    init() {
      var self = this;

      // Listen for OS theme changes (only matters when mode is 'system')
      window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', function(e) {
        if (self.themeMode === 'system') {
          self.theme = e.matches ? 'dark' : 'light';
        }
      });

      // Hash routing
      var validPages = ['overview','agents','live','starnet','artifacts','projects','teams','sessions','approvals','comms','workflows','scheduler','channels','skills','hands','analytics','logs','runtime','settings','wizard'];
      var pageRedirects = {
        'chat': 'agents',
        'templates': 'agents',
        'triggers': 'workflows',
        'cron': 'scheduler',
        'schedules': 'scheduler',
        'memory': 'sessions',
        'audit': 'logs',
        'security': 'settings',
        'peers': 'settings',
        'migration': 'settings',
        'usage': 'analytics',
        'approval': 'approvals'
      };
      function handleHash() {
        var hash = window.location.hash.replace('#', '') || 'agents';
        if (pageRedirects[hash]) {
          hash = pageRedirects[hash];
          window.location.hash = hash;
        }
        if (validPages.indexOf(hash) >= 0) self.page = hash;
      }
      window.addEventListener('hashchange', handleHash);
      handleHash();

      // Keyboard shortcuts
      document.addEventListener('keydown', function(e) {
        // Ctrl+K — focus agent switch / go to agents
        if ((e.ctrlKey || e.metaKey) && e.key === 'k') {
          e.preventDefault();
          self.navigate('agents');
        }
        // Ctrl+N — new agent
        if ((e.ctrlKey || e.metaKey) && e.key === 'n' && !e.shiftKey) {
          e.preventDefault();
          self.navigate('agents');
        }
        // Ctrl+Shift+F — toggle focus mode
        if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key === 'F') {
          e.preventDefault();
          Alpine.store('app').toggleFocusMode();
        }
        // Escape — close mobile menu
        if (e.key === 'Escape') {
          self.mobileMenuOpen = false;
        }
      });

      // Connection state listener
      FreecoAPI.onConnectionChange(function(state) {
        Alpine.store('app').connectionState = state;
      });

      // Initial data load
      this.pollStatus();
      Alpine.store('app').refreshApprovals();
      Alpine.store('app').refreshFreeze();
      Alpine.store('app').checkOnboarding();
      Alpine.store('app').checkAuth();
      setInterval(function() {
        self.pollStatus();
        Alpine.store('app').refreshApprovals();
        Alpine.store('app').refreshFreeze();
      }, 5000);
    },

    navigate(p) {
      this.page = p;
      window.location.hash = p;
      this.mobileMenuOpen = false;
    },

    setTheme(mode) {
      this.themeMode = mode;
      localStorage.setItem('freeco-theme-mode', mode);
      if (mode === 'system') {
        this.theme = window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
      } else {
        this.theme = mode;
      }
    },

    toggleTheme() {
      var modes = ['light', 'system', 'dark'];
      var next = modes[(modes.indexOf(this.themeMode) + 1) % modes.length];
      this.setTheme(next);
    },

    toggleSidebar() {
      this.sidebarCollapsed = !this.sidebarCollapsed;
      localStorage.setItem('freeco-sidebar', this.sidebarCollapsed ? 'collapsed' : 'expanded');
    },

    async pollStatus() {
      var store = Alpine.store('app');
      await store.checkStatus();
      await store.refreshAgents();
      this.connected = store.connected;
      this.version = store.version;
      this.agentCount = store.agentCount;
      this.wsConnected = FreecoAPI.isWsConnected();
    }
  };
}

// Dictation settings, shared by the chat page and the assistant widget.
//
// The language used to come from navigator.language. A browser's recogniser
// only offers certain languages, and one it does not offer fails with
// `language-not-supported` -- which is what the locale of a machine set to a
// language the vendor does not serve produces, every time, with no way for the
// user to choose otherwise.
//
// `browserSttBroken` records that this browser's recogniser cannot be used at
// all. Brave defines SpeechRecognition but removed the vendor key behind it, so
// it is present, always fails, and the "no recogniser here" fallback that keyed
// off its absence could never run. Once it has failed for a reason that will
// not change, dictation goes straight to record-and-upload instead of failing
// again on every press.
window.FreecoVoiceCfg = {
  // Languages offered in Settings. 'Browser default' keeps the old behaviour.
  LANGUAGES: [
    { code: '', label: 'Browser default' },
    { code: 'en-US', label: 'English (US)' },
    { code: 'en-GB', label: 'English (UK)' },
    { code: 'uk-UA', label: 'Українська' },
    { code: 'fr-FR', label: 'Français' },
    { code: 'de-DE', label: 'Deutsch' },
    { code: 'it-IT', label: 'Italiano' }
  ],
  lang: function () {
    try {
      return localStorage.getItem('freeco-stt-lang') || navigator.language || 'en-US';
    } catch (e) { return navigator.language || 'en-US'; }
  },
  savedLang: function () {
    try { return localStorage.getItem('freeco-stt-lang') || ''; } catch (e) { return ''; }
  },
  setLang: function (v) {
    try {
      if (v) { localStorage.setItem('freeco-stt-lang', v); }
      else { localStorage.removeItem('freeco-stt-lang'); }
      // A different language may well be one this browser can serve.
      localStorage.removeItem('freeco-stt-browser-broken');
    } catch (e) { /* private mode */ }
  },
  browserSttBroken: function () {
    try { return localStorage.getItem('freeco-stt-browser-broken') === '1'; } catch (e) { return false; }
  },
  // Errors that mean this browser will never transcribe, however many times it
  // is asked: no vendor service, or no support for the chosen language.
  isPermanentSttError: function (why) {
    return why === 'language-not-supported' || why === 'service-not-allowed' ||
           why === 'network' || why === 'not-supported';
  },
  markBrowserSttBroken: function () {
    try { localStorage.setItem('freeco-stt-browser-broken', '1'); } catch (e) { /* private mode */ }
  },
  // Text as it should be heard. Replies are Markdown; read raw, every `**`
  // came out as "asterisk asterisk". Shared by the main chat and the popup.
  cleanForSpeech: function (text) {
    return String(text || '')
      .replace(/```[\s\S]*?```/g, ' code block ')      // don't read code aloud
      .replace(/`([^`]*)`/g, '$1')
      .replace(/!\[[^\]]*\]\([^)]*\)/g, '')             // images
      .replace(/\[([^\]]+)\]\([^)]+\)/g, '$1')          // links -> link text
      .replace(/^\s{0,3}[-*+]\s+/gm, '')                // list bullets
      .replace(/^\s{0,3}\d+[.)]\s+/gm, '')              // numbered list markers
      .replace(/^\s*\|?[\s:|-]+\|[\s:|-]*$/gm, '')      // table separator rows
      .replace(/[*_~#>|]/g, ' ')
      .replace(/https?:\/\/\S+/g, 'link')
      .replace(/\s+/g, ' ')
      .trim();
  },
  // The voice to speak with: the one chosen in the assistant's settings, else
  // the best local voice for the language. "Natural" voices first -- the
  // browser default is the robotic one. Voices that send text off the machine
  // ("Online") are never picked automatically.
  pickVoice: function () {
    if (!window.speechSynthesis) return null;
    var voices = window.speechSynthesis.getVoices() || [];
    if (!voices.length) return null;
    var chosen = '';
    try { chosen = localStorage.getItem('freeco-voice-name') || ''; } catch (e) { /* private mode */ }
    if (chosen) {
      var hit = voices.find(function (v) { return v.name === chosen; });
      if (hit) return hit;
    }
    var local = voices.filter(function (v) {
      return v.localService !== false && !/\bonline\b/i.test(v.name || '');
    });
    var pool = local.length ? local : voices;
    var lang = (this.lang() || navigator.language || 'en-US').toLowerCase();
    var base = lang.split('-')[0];
    var sameLang = pool.filter(function (v) { return (v.lang || '').toLowerCase() === lang; });
    if (!sameLang.length) {
      sameLang = pool.filter(function (v) { return (v.lang || '').toLowerCase().split('-')[0] === base; });
    }
    var cands = sameLang.length ? sameLang : pool;
    return cands.find(function (v) { return /natural|neural/i.test(v.name || ''); }) || cands[0];
  }
};

// Open a specific agent's chat from anywhere.
//
// The assistant can hand a message to another agent's conversation, and a
// message delivered somewhere the user cannot reach is worse than one not
// delivered: they are told it was sent and have no way to see it. The chat page
// already opens whatever `pendingAgent` names, so this points that at the agent
// and switches to the chat.
//
// It goes to `agents` itself, not the `chat` alias: the alias is a page that
// does not exist, so going through it unmounts an open chat for a moment and
// the pending agent is spent on the chat being torn down. The user then lands
// on the agent list instead of the chat they asked for.
window.addEventListener('freeco-open-agent', function (ev) {
  var id = ev && ev.detail;
  if (!id || typeof Alpine === 'undefined') return;
  var store = Alpine.store('app');
  if (!store) return;
  var agent = (store.agents || []).filter(function (a) { return a.id === id; })[0];
  if (agent) store.pendingAgent = agent;
  window.dispatchEvent(new CustomEvent('freeco-navigate', { detail: 'agents' }));
});

// Deep links: /?ask=...&agent=...
//
// This is what makes the assistant's action reachable from outside the
// dashboard. A link dropped into a note, a chat, or another device opens
// FreEco.ai with the text already in the box, so the thing carried across is a
// working request rather than a reminder to retype one. The text is only ever
// placed in the input -- never sent -- because a link that acts on open would
// let any page act as the user.
(function () {
  function readDeepLink() {
    var p;
    try { p = new URLSearchParams(window.location.search); } catch (e) { return null; }
    var ask = p.get('ask');
    var agent = p.get('agent');
    if (!ask && !agent) return null;
    return { ask: ask || '', agent: agent || '' };
  }
  var link = readDeepLink();
  if (!link) return;
  // Drop the parameters so a refresh does not repeat this.
  try {
    window.history.replaceState({}, '', window.location.pathname + window.location.hash);
  } catch (e) { /* ignore */ }
  window.addEventListener('DOMContentLoaded', function () {
    setTimeout(function () {
      if (link.agent) {
        window.dispatchEvent(new CustomEvent('freeco-open-agent', { detail: link.agent }));
        if (link.ask) window.dispatchEvent(new CustomEvent('freeco-prefill-chat', { detail: link.ask }));
      } else {
        window.dispatchEvent(new CustomEvent('freeco-open'));
        if (link.ask) window.dispatchEvent(new CustomEvent('freeco-prefill', { detail: link.ask }));
      }
    }, 400);
  });
})();

// Widget mode: /?widget=1 shows only the assistant, filling its own window.
//
// The desktop "FreEco Assistant" shortcut opens this in a small app window, so
// the assistant can sit beside other work without the whole dashboard. The
// parameter stays in the address (unlike a deep link) so a reload keeps the
// mode. Styling is in components.css under `html.widget-mode`.
(function () {
  var widget = false;
  try { widget = new URLSearchParams(window.location.search).get('widget') === '1'; } catch (e) { /* ignore */ }
  if (!widget) return;
  document.documentElement.classList.add('widget-mode');
  try { document.title = 'FreEco Assistant'; } catch (e) { /* ignore */ }
  window.addEventListener('DOMContentLoaded', function () {
    setTimeout(function () { window.dispatchEvent(new CustomEvent('freeco-open')); }, 300);
  });
})();

// Keep secrets out of conversations.
//
// A key pasted into a chat goes to the model provider with the message and is
// resent with the history on every later call. Before any chat or popup
// message is sent it is checked here; if it holds a key, the user is offered
// to store it in the vault instead, and the message goes out with the key
// replaced by its vault name. check() resolves to the text to send, or null
// if the user chose to go back and edit.
window.FreecoSecretGuard = (function () {
  function esc(s) {
    return String(s).replace(/[&<>"']/g, function (c) {
      return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
    });
  }

  function ask(found) {
    return new Promise(function (resolve) {
      var overlay = document.createElement('div');
      overlay.className = 'modal-overlay';
      var rows = found.map(function (f, i) {
        return '<label style="display:block;margin:10px 0 4px;font-size:12px;color:var(--text-secondary)">' +
          'Key ' + (i + 1) + ' (' + esc(f.masked) + ') is taken from your message. ' +
          'Name to save it under (the name only, not the key):</label>' +
          '<input class="form-input" data-i="' + i + '" value="' + esc(f.suggested_name) + '" ' +
          'autocomplete="off" spellcheck="false" style="width:100%;font-family:var(--font-mono,monospace)">' +
          // Saving over a stored key replaces it -- say so, never do it quietly.
          (f.exists ? '<div style="font-size:12px;color:var(--warning,#b45309);margin-top:3px">' +
            esc(f.suggested_name) + ' already holds a key. Saving replaces it; ' +
            'change the name to keep both.</div>' : '');
      }).join('');
      overlay.innerHTML =
        '<div class="modal" role="dialog" aria-modal="true" style="max-width:480px">' +
        '<div class="modal-header"><h3>This message contains a key</h3></div>' +
        '<p style="font-size:13px;margin:0 0 6px">Sent as it is, the key would go to the model provider ' +
        'and be resent with every later message in this chat.</p>' +
        '<p style="font-size:13px;margin:0">Save it in the vault instead. The message is sent with the key ' +
        'replaced by its name, and the tools that need it read it from the vault.</p>' +
        rows +
        '<div class="fsg-err" style="color:var(--error);font-size:12px;margin-top:8px"></div>' +
        '<div style="display:flex;gap:8px;justify-content:flex-end;margin-top:16px">' +
        '<button class="btn btn-ghost btn-sm fsg-cancel">Back to editing</button>' +
        '<button class="btn btn-primary btn-sm fsg-save">Save in vault and send</button>' +
        '</div></div>';
      document.body.appendChild(overlay);
      var saveBtn = overlay.querySelector('.fsg-save');
      saveBtn.focus();
      function done(v) { overlay.remove(); resolve(v); }
      overlay.querySelector('.fsg-cancel').onclick = function () { done(null); };
      overlay.addEventListener('keydown', function (e) { if (e.key === 'Escape') done(null); });
      saveBtn.onclick = function () {
        var raw = Array.prototype.map.call(overlay.querySelectorAll('input[data-i]'), function (el) {
          return el.value.trim();
        });
        // The box reads like a place to put the key, and the key was pasted
        // into it. Say that rather than only quoting the naming rule.
        var keyInName = raw.some(function (n, i) {
          return found[i].value.length > 8 && n.indexOf(found[i].value.slice(0, 8)) !== -1;
        });
        if (keyInName) {
          overlay.querySelector('.fsg-err').textContent =
            'This box is for the name only: the key is already taken from your message. ' +
            'Leave just the name in it, for example ' + found[0].suggested_name + '.';
          return;
        }
        var names = raw.map(function (n) { return n.toUpperCase(); });
        var bad = names.filter(function (n) { return !/^[A-Z][A-Z0-9_]{1,63}$/.test(n); });
        if (bad.length) {
          overlay.querySelector('.fsg-err').textContent =
            'Names use capital letters, digits and _, starting with a letter (for example CRM_API_KEY).';
          return;
        }
        // Two keys under one name: the second save would overwrite the first.
        var dup = names.filter(function (n, i) { return names.indexOf(n) !== i; });
        if (dup.length) {
          overlay.querySelector('.fsg-err').textContent =
            'Each key needs its own name: ' + dup[0] + ' is used twice.';
          return;
        }
        done(names);
      };
    });
  }

  async function check(text) {
    if (!text || text.length < 16) return text;
    var found;
    try {
      found = (await FreecoAPI.post('/api/gateway/scan', { text: text })).secrets || [];
    } catch (e) {
      // The gateway still masks keys on the way out; failing to ask here must
      // not stop the user from sending.
      return text;
    }
    if (!found.length) return text;
    found.forEach(function (f) {
      var v = text.slice(f.start, f.end);
      f.value = v;
      f.masked = v.length > 10 ? v.slice(0, 4) + '…' + v.slice(-3) : '…';
    });
    var names = await ask(found);
    if (!names) return null;
    // Save each, then replace from the end so earlier offsets stay valid.
    for (var i = 0; i < found.length; i++) {
      try {
        await FreecoAPI.post('/api/vault/secrets', { name: names[i], value: found[i].value });
      } catch (e) {
        FreecoToast.error('Could not save the key: ' + (e.message || 'error') + '. Nothing was sent.');
        return null;
      }
    }
    var out = text;
    for (var j = found.length - 1; j >= 0; j--) {
      out = out.slice(0, found[j].start) + '[' + names[j] + ' saved in the vault]' + out.slice(found[j].end);
    }
    FreecoToast.success('Saved in the vault: ' + names.join(', '));
    return out;
  }

  return { check: check };
})();

// Get the user's attention when an agent is blocked on them.
//
// A toast is visible only while the dashboard is the window in front. An agent
// waiting on a shell approval with the dashboard behind another window waits
// indefinitely, and the user concludes the agent is slow. A short tone and a
// desktop notification reach them wherever they are.
window.FreecoAlert = (function () {
  var ctx = null;
  function muted() {
    try { return localStorage.getItem('freeco-alert-sound') === 'off'; } catch (e) { return false; }
  }
  // Browsers only allow audio and notification permission after a user gesture,
  // so prepare both on the first click anywhere instead of failing silently
  // the first time they are needed.
  function prime() {
    document.removeEventListener('click', prime, true);
    try {
      var AC = window.AudioContext || window.webkitAudioContext;
      if (AC && !ctx) ctx = new AC();
    } catch (e) { /* no audio here */ }
    try {
      if ('Notification' in window && Notification.permission === 'default') {
        Notification.requestPermission();
      }
    } catch (e) { /* no notifications here */ }
  }
  document.addEventListener('click', prime, true);

  return {
    // Two short rising tones: noticeable, not alarming.
    chime: function () {
      if (muted() || !ctx) return;
      try {
        if (ctx.state === 'suspended') ctx.resume();
        [660, 880].forEach(function (freq, i) {
          var t0 = ctx.currentTime + i * 0.16;
          var osc = ctx.createOscillator();
          var gain = ctx.createGain();
          osc.type = 'sine';
          osc.frequency.value = freq;
          gain.gain.setValueAtTime(0.0001, t0);
          gain.gain.exponentialRampToValueAtTime(0.18, t0 + 0.02);
          gain.gain.exponentialRampToValueAtTime(0.0001, t0 + 0.14);
          osc.connect(gain); gain.connect(ctx.destination);
          osc.start(t0); osc.stop(t0 + 0.15);
        });
      } catch (e) { /* sound is a courtesy, never an error */ }
    },
    // Only when the page is not already in front: a notification on top of
    // the window the user is looking at is noise.
    notify: function (title, body, page) {
      try {
        if (!('Notification' in window) || Notification.permission !== 'granted') return;
        if (document.visibilityState === 'visible' && document.hasFocus()) return;
        var n = new Notification(title, { body: body, tag: 'freeco-approval' });
        n.onclick = function () {
          window.focus();
          if (page) window.dispatchEvent(new CustomEvent('freeco-navigate', { detail: page }));
          n.close();
        };
      } catch (e) { /* ignore */ }
    },
    setSound: function (on) {
      try { localStorage.setItem('freeco-alert-sound', on ? 'on' : 'off'); } catch (e) { /* ignore */ }
    },
    soundOn: function () { return !muted(); }
  };
})();
