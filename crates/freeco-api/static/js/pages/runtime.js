// Runtime page — system overview and provider status
document.addEventListener('alpine:init', function() {
  Alpine.data('runtimePage', function() {
    return {
      loading: true,
      uptime: '-',
      agentCount: 0,
      version: '-',
      defaultModel: '-',
      platform: '-',
      arch: '-',
      apiListen: '-',
      homeDir: '-',
      logLevel: '-',
      networkEnabled: false,
      providers: [],
      company: { teams: [], workflows: [], approvals: { security_pending: 0 } },
      backupStatus: '',
      backups: [],
      restoreBusy: false,
      restoreResult: null,

      // --- Default model picker ---
      showDefaultModelPicker: false,
      defaultModelQuery: '',
      defaultModelList: [],
      defaultModelSaving: false,
      defaultModelMsg: '',
      defaultModelMsgOk: false,

      // Only models the user can actually run: their provider has a key, and
      // they take tools. Offering a model whose provider is unconfigured
      // produces an auth error at the first message, blamed on the model.
      get filteredDefaultModels() {
        var configured = {};
        (this.providers || []).forEach(function(p) {
          if (p.auth_status === 'configured' || p.auth_status === 'not_required') {
            configured[p.id] = true;
          }
        });
        var q = (this.defaultModelQuery || '').toLowerCase();
        return (this.defaultModelList || []).filter(function(m) {
          if (!configured[m.provider]) return false;
          if (m.supports_tools === false) return false;
          if (!q) return true;
          return (m.id || '').toLowerCase().indexOf(q) !== -1 ||
                 (m.display_name || '').toLowerCase().indexOf(q) !== -1 ||
                 (m.provider || '').toLowerCase().indexOf(q) !== -1;
        }).sort(function(a, b) {
          // Free first, then by name, so the zero-cost options are the ones
          // a newcomer sees without having to know which those are.
          if (!!b.is_free !== !!a.is_free) return b.is_free ? 1 : -1;
          return (a.id || '').localeCompare(b.id || '');
        }).slice(0, 200);
      },

      async openDefaultModelPicker() {
        this.showDefaultModelPicker = true;
        this.defaultModelMsg = '';
        if (this.defaultModelList.length) return;
        try {
          var data = await FreecoAPI.get('/api/models');
          this.defaultModelList = Array.isArray(data) ? data : (data.models || []);
        } catch (e) {
          this.defaultModelMsg = 'Could not load the model list.';
          this.defaultModelMsgOk = false;
        }
      },

      async chooseDefaultModel(m) {
        this.defaultModelSaving = true;
        this.defaultModelMsg = '';
        try {
          var res = await FreecoAPI.put('/api/config/default-model',
                                        { model: m.id, provider: m.provider });
          if (res && (res.status === 'ok' || res.status === 'applied')) {
            this.defaultModel = res.provider + ' : ' + res.model;
            this.showDefaultModelPicker = false;
            // A change that is active but unsaved is reported as such rather
            // than as a clean success, because it will not survive a restart.
            this.defaultModelMsg = res.warning || 'Default model updated.';
            this.defaultModelMsgOk = !res.warning;
          } else {
            this.defaultModelMsg = (res && res.error) || 'Could not change the default model.';
            this.defaultModelMsgOk = false;
          }
        } catch (e) {
          this.defaultModelMsg = 'Could not change the default model.';
          this.defaultModelMsgOk = false;
        }
        this.defaultModelSaving = false;
      },

      async loadData() {
        this.loading = true;
        try {
          var results = await Promise.all([
            FreecoAPI.get('/api/status'),
            FreecoAPI.get('/api/version'),
            FreecoAPI.get('/api/providers'),
            FreecoAPI.get('/api/agents'),
            FreecoAPI.get('/api/company-chart')
          ]);
          var status = results[0];
          var ver = results[1];
          var prov = results[2];
          var agents = results[3];
          this.company = results[4] || this.company;

          this.version = ver.version || '-';
          this.platform = ver.platform || '-';
          this.arch = ver.arch || '-';
          this.agentCount = Array.isArray(agents) ? agents.length : 0;
          this.defaultModel = status.default_model || '-';
          this.apiListen = status.api_listen || status.listen || '-';
          this.homeDir = status.home_dir || '-';
          this.logLevel = status.log_level || '-';
          this.networkEnabled = !!status.network_enabled;

          // Compute uptime from uptime_seconds
          var diff = status.uptime_seconds || 0;
          if (diff < 60) this.uptime = diff + 's';
          else if (diff < 3600) this.uptime = Math.floor(diff / 60) + 'm ' + (diff % 60) + 's';
          else if (diff < 86400) this.uptime = Math.floor(diff / 3600) + 'h ' + Math.floor((diff % 3600) / 60) + 'm';
          else this.uptime = Math.floor(diff / 86400) + 'd ' + Math.floor((diff % 86400) / 3600) + 'h';

          this.providers = (prov.providers || []).filter(function(p) {
            return p.auth_status === 'Configured' || p.reachable || p.is_local;
          });
        } catch(e) {
          console.error('Runtime load error:', e);
        }
        this.loading = false;
      },

      async createBackup() {
        this.backupStatus = 'Creating encrypted backup...';
        try {
          var result = await FreecoAPI.post('/api/backups', { retention: 7 });
          this.backupStatus = 'Backup created: ' + (result.files || '') + ' files';
          FreecoToast.success(this.backupStatus);
          await this.loadBackups();
        } catch (e) {
          this.backupStatus = 'Backup failed';
          FreecoToast.error('Encrypted backup failed');
        }
      },

      async loadBackups() {
        try {
          var data = await FreecoAPI.get('/api/backups');
          this.backups = (data.backups || []).map(function(b) {
            return {
              name: b.name,
              sizeMb: (b.size / 1048576).toFixed(1),
              when: b.modified ? new Date(b.modified * 1000).toLocaleString() : ''
            };
          });
        } catch (e) { this.backups = []; }
      },

      // Verify a backup first (dry run) — never destructive.
      async verifyRestore(name) {
        this.restoreBusy = true;
        this.restoreResult = null;
        try {
          var r = await FreecoAPI.post('/api/backups/restore', { archive_name: name, dry_run: true });
          this.restoreResult = { name: name, ok: true, detail: 'Verified: ' + (r.files || 0) + ' files, archive is intact and decryptable.' };
        } catch (e) {
          this.restoreResult = { name: name, ok: false, detail: 'Verify failed: ' + (e.message || 'archive unreadable') };
        }
        this.restoreBusy = false;
      },

      restoreBackup(name) {
        var self = this;
        FreecoToast.confirm(
          'Restore backup',
          'Restore "' + name + '"? This overwrites current data with the archive contents. A restart is needed afterwards. Verify first if unsure.',
          async function() {
            self.restoreBusy = true;
            try {
              await FreecoAPI.post('/api/backups/restore', { archive_name: name, dry_run: false });
              FreecoToast.success('Restore complete — restart FreEco.ai to load the restored data.');
              self.restoreResult = { name: name, ok: true, detail: 'Restored. Restart to apply.' };
            } catch (e) {
              FreecoToast.error('Restore failed: ' + (e.message || 'error'));
            }
            self.restoreBusy = false;
          }
        );
      }
    };
  });
});
