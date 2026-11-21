// FreEco.ai Channels Page — OpenClaw-style setup UX with QR code support
'use strict';

function channelsPage() {
  return {
    allChannels: [],
    categoryFilter: 'all',
    searchQuery: '',
    setupModal: null,
    configuring: false,
    testing: {},
    formValues: {},
    showAdvanced: false,
    showBusinessApi: false,
    loading: true,
    loadError: '',
    pollTimer: null,

    // Setup flow step tracking
    setupStep: 1, // 1=Configure, 2=Verify, 3=Ready
    testPassed: false,

    // WhatsApp QR state
    qr: {
      loading: false,
      available: false,
      dataUrl: '',
      sessionId: '',
      message: '',
      help: '',
      connected: false,
      expired: false,
      error: ''
    },
    qrPollTimer: null,

    // WhatsApp allowlist + gateway status (GET /api/channels/whatsapp/status)
    wa: { loading: false, data: null, error: '', saving: false },
    waPollTimer: null,

    categories: [
      { key: 'all', label: 'All' },
      { key: 'messaging', label: 'Messaging' },
      { key: 'social', label: 'Social' },
      { key: 'enterprise', label: 'Enterprise' },
      { key: 'developer', label: 'Developer' },
      { key: 'notifications', label: 'Notifications' }
    ],

    get filteredChannels() {
      var self = this;
      return this.allChannels.filter(function(ch) {
        if (self.categoryFilter !== 'all' && ch.category !== self.categoryFilter) return false;
        if (self.searchQuery) {
          var q = self.searchQuery.toLowerCase();
          return ch.name.toLowerCase().indexOf(q) !== -1 ||
                 ch.display_name.toLowerCase().indexOf(q) !== -1 ||
                 ch.description.toLowerCase().indexOf(q) !== -1;
        }
        return true;
      });
    },

    get configuredCount() {
      return this.allChannels.filter(function(ch) { return ch.configured; }).length;
    },

    categoryCount(cat) {
      var all = this.allChannels.filter(function(ch) { return cat === 'all' || ch.category === cat; });
      var configured = all.filter(function(ch) { return ch.configured; });
      return configured.length + '/' + all.length;
    },

    basicFields() {
      if (!this.setupModal || !this.setupModal.fields) return [];
      return this.setupModal.fields.filter(function(f) { return !f.advanced; });
    },

    advancedFields() {
      if (!this.setupModal || !this.setupModal.fields) return [];
      return this.setupModal.fields.filter(function(f) { return f.advanced; });
    },

    hasAdvanced() {
      return this.advancedFields().length > 0;
    },

    isQrChannel() {
      return this.setupModal && this.setupModal.setup_type === 'qr';
    },

    async loadChannels() {
      this.loading = true;
      this.loadError = '';
      try {
        var data = await FreecoAPI.get('/api/channels');
        this.allChannels = (data.channels || []).map(function(ch) {
          ch.connected = ch.configured && ch.has_token;
          return ch;
        });
      } catch(e) {
        this.loadError = e.message || 'Could not load channels.';
      }
      this.loading = false;
      this.startPolling();
    },

    async loadData() { return this.loadChannels(); },

    startPolling() {
      var self = this;
      if (this.pollTimer) clearInterval(this.pollTimer);
      this.pollTimer = setInterval(function() { self.refreshStatus(); }, 15000);
    },

    async refreshStatus() {
      try {
        var data = await FreecoAPI.get('/api/channels');
        var byName = {};
        (data.channels || []).forEach(function(ch) { byName[ch.name] = ch; });
        this.allChannels.forEach(function(c) {
          var fresh = byName[c.name];
          if (fresh) {
            c.configured = fresh.configured;
            c.has_token = fresh.has_token;
            c.connected = fresh.configured && fresh.has_token;
            c.fields = fresh.fields;
          }
        });
      } catch(e) { console.warn('Channel refresh failed:', e.message); }
    },

    statusBadge(ch) {
      if (!ch.configured) return { text: 'Not Configured', cls: 'badge-muted' };
      if (!ch.has_token) return { text: 'Missing Token', cls: 'badge-warn' };
      if (ch.connected) return { text: 'Ready', cls: 'badge-success' };
      return { text: 'Configured', cls: 'badge-info' };
    },

    difficultyClass(d) {
      if (d === 'Easy') return 'difficulty-easy';
      if (d === 'Hard') return 'difficulty-hard';
      return 'difficulty-medium';
    },

    openSetup(ch) {
      this.setupModal = ch;
      // Pre-populate form values from saved config (non-secret fields).
      var vals = {};
      if (ch.fields) {
        ch.fields.forEach(function(f) {
          if (f.value !== undefined && f.value !== null && f.type !== 'secret') {
            vals[f.key] = String(f.value);
          }
        });
      }
      this.formValues = vals;
      this.showAdvanced = false;
      this.showBusinessApi = false;
      this.setupStep = ch.configured ? 3 : 1;
      this.testPassed = !!ch.configured;
      this.resetQR();
      if (ch.name === 'whatsapp') this.watchWhatsApp();
      // Auto-start QR flow for QR-type channels
      if (ch.setup_type === 'qr') {
        this.startQR();
      }
    },

    // ── WhatsApp: allowlist and gateway status ─────────────────────

    async loadWhatsAppStatus() {
      this.wa.loading = !this.wa.data;
      try {
        this.wa.data = await FreecoAPI.get('/api/channels/whatsapp/status');
        this.wa.error = '';
      } catch(e) {
        this.wa.error = e.message || 'Could not load the WhatsApp status';
      }
      this.wa.loading = false;
    },

    // Refresh while the WhatsApp settings are open; stops by itself when closed.
    watchWhatsApp() {
      var self = this;
      this.wa = { loading: false, data: null, error: '', saving: false };
      if (this.waPollTimer) clearInterval(this.waPollTimer);
      this.loadWhatsAppStatus();
      this.waPollTimer = setInterval(function() {
        if (!self.setupModal || self.setupModal.name !== 'whatsapp') {
          clearInterval(self.waPollTimer);
          self.waPollTimer = null;
          return;
        }
        self.loadWhatsAppStatus();
      }, 5000);
    },

    waStateLabel() {
      var st = this.wa.data && this.wa.data.state;
      var labels = {
        not_configured: { text: 'Not set up', cls: 'badge-muted' },
        connected: { text: 'Connected', cls: 'badge-success' },
        qr_needed: { text: 'QR scan needed', cls: 'badge-warn' },
        connecting: { text: 'Connecting', cls: 'badge-info' },
        starting: { text: 'Gateway starting', cls: 'badge-info' },
        restarting: { text: 'Gateway restarting', cls: 'badge-warn' },
        gave_up: { text: 'Gateway stopped after repeated crashes', cls: 'badge-error' },
        node_missing: { text: 'Node.js not found', cls: 'badge-error' },
        install_failed: { text: 'Gateway install failed', cls: 'badge-error' },
        unreachable: { text: 'Gateway not answering', cls: 'badge-error' },
        stopped: { text: 'Gateway stopped', cls: 'badge-muted' }
      };
      return labels[st] || { text: st ? String(st) : 'Unknown', cls: 'badge-muted' };
    },

    waList(kind) {
      var list = this.wa.data && this.wa.data.allowlist;
      return (list && Array.isArray(list[kind])) ? list[kind] : [];
    },

    // Save only who may talk to the agent; the daemon restarts the gateway
    // with the new list (and starts it, the first time).
    async saveWhatsAppAllowlist() {
      if (this.wa.saving) return;
      this.wa.saving = true;
      try {
        var res = await FreecoAPI.post('/api/channels/whatsapp/configure', { fields: this.formValues });
        FreecoToast.success('WhatsApp allowlist saved. ' + ((res && res.note) || ''));
        await this.loadWhatsAppStatus();
        await this.refreshStatus();
        var self = this;
        if (!this.qr.connected) setTimeout(function() { if (self.setupModal && self.setupModal.name === 'whatsapp') self.startQR(); }, 3000);
      } catch(e) {
        FreecoToast.error('Failed: ' + (e.message || 'Unknown error'));
      }
      this.wa.saving = false;
    },

    // ── QR Code Flow (WhatsApp Web style) ──────────────────────────

    resetQR() {
      this.qr = {
        loading: false, available: false, dataUrl: '', sessionId: '',
        message: '', help: '', connected: false, expired: false, error: ''
      };
      if (this.qrPollTimer) { clearInterval(this.qrPollTimer); this.qrPollTimer = null; }
    },

    async startQR() {
      this.qr.loading = true;
      this.qr.error = '';
      this.qr.connected = false;
      this.qr.expired = false;
      try {
        var result = await FreecoAPI.post('/api/channels/whatsapp/qr/start', {});
        this.qr.available = result.available || false;
        this.qr.dataUrl = result.qr_data_url || '';
        this.qr.sessionId = result.session_id || '';
        this.qr.message = result.message || '';
        this.qr.help = result.help || '';
        this.qr.connected = result.connected || false;
        if (this.qr.available && this.qr.dataUrl && !this.qr.connected) {
          this.pollQR();
        }
        if (this.qr.connected) {
          FreecoToast.success('WhatsApp connected!');
          await this.refreshStatus();
        }
      } catch(e) {
        this.qr.error = e.message || 'Could not start QR login';
      }
      this.qr.loading = false;
    },

    pollQR() {
      var self = this;
      if (this.qrPollTimer) clearInterval(this.qrPollTimer);
      this.qrPollTimer = setInterval(async function() {
        try {
          var result = await FreecoAPI.get('/api/channels/whatsapp/qr/status?session_id=' + encodeURIComponent(self.qr.sessionId));
          if (result.connected) {
            clearInterval(self.qrPollTimer);
            self.qrPollTimer = null;
            self.qr.connected = true;
            self.qr.message = result.message || 'Connected!';
            FreecoToast.success('WhatsApp linked successfully!');
            await self.refreshStatus();
          } else if (result.expired) {
            clearInterval(self.qrPollTimer);
            self.qrPollTimer = null;
            self.qr.expired = true;
            self.qr.message = 'QR code expired. Click to generate a new one.';
          } else {
            self.qr.message = result.message || 'Waiting for scan...';
          }
        } catch(e) { /* silent retry */ }
      }, 3000);
    },

    // ── Standard Form Flow ─────────────────────────────────────────

    async saveChannel() {
      if (!this.setupModal) return;
      var name = this.setupModal.name;
      this.configuring = true;
      try {
        await FreecoAPI.post('/api/channels/' + name + '/configure', {
          fields: this.formValues
        });
        this.setupStep = 2;
        // Auto-test after save
        try {
          var testResult = await FreecoAPI.post('/api/channels/' + name + '/test', {});
          if (testResult.status === 'ok') {
            this.testPassed = true;
            this.setupStep = 3;
            FreecoToast.success(this.setupModal.display_name + ' activated!');
          } else {
            FreecoToast.success(this.setupModal.display_name + ' saved. ' + (testResult.message || ''));
          }
        } catch(te) {
          FreecoToast.success(this.setupModal.display_name + ' saved. Test to verify connection.');
        }
        await this.refreshStatus();
      } catch(e) {
        FreecoToast.error('Failed: ' + (e.message || 'Unknown error'));
      }
      this.configuring = false;
    },

    async removeChannel() {
      if (!this.setupModal) return;
      var name = this.setupModal.name;
      var displayName = this.setupModal.display_name;
      var self = this;
      FreecoToast.confirm('Remove Channel', 'Remove ' + displayName + ' configuration? This will deactivate the channel.', async function() {
        try {
          await FreecoAPI.delete('/api/channels/' + name + '/configure');
          FreecoToast.success(displayName + ' removed and deactivated.');
          await self.refreshStatus();
          self.setupModal = null;
        } catch(e) {
          FreecoToast.error('Failed: ' + (e.message || 'Unknown error'));
        }
      });
    },

    async testChannel() {
      if (!this.setupModal) return;
      var name = this.setupModal.name;
      this.testing[name] = true;
      try {
        var result = await FreecoAPI.post('/api/channels/' + name + '/test', {});
        if (result.status === 'ok') {
          this.testPassed = true;
          this.setupStep = 3;
          FreecoToast.success(result.message);
        } else {
          FreecoToast.error(result.message);
        }
      } catch(e) {
        FreecoToast.error('Test failed: ' + (e.message || 'Unknown error'));
      }
      this.testing[name] = false;
    },

    async copyConfig(ch) {
      var tpl = ch ? ch.config_template : (this.setupModal ? this.setupModal.config_template : '');
      if (!tpl) return;
      try {
        await navigator.clipboard.writeText(tpl);
        FreecoToast.success('Copied to clipboard');
      } catch(e) {
        FreecoToast.error('Copy failed');
      }
    },

    destroy() {
      if (this.pollTimer) { clearInterval(this.pollTimer); this.pollTimer = null; }
      if (this.qrPollTimer) { clearInterval(this.qrPollTimer); this.qrPollTimer = null; }
      if (this.waPollTimer) { clearInterval(this.waPollTimer); this.waPollTimer = null; }
    }
  };
}
