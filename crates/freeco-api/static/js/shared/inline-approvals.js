// Permission requests, answered where the user already is.
//
// This lived only in the floating assistant widget, so anyone working in the
// Chat page saw their task simply stop: the agent was waiting on an approval
// that was rendered somewhere they were not looking. They then had to find the
// Approvals page to unblock work they were in the middle of.
//
// A factory rather than a shared object, because each chat surface needs its
// own `approvals` array -- a single object literal would share one list between
// them by reference, and answering in one place would empty the other.
window.FreecoInlineApprovals = function () {
  return {
    approvals: [],
    _approvalTimer: null,

    startApprovalPolling: function (intervalMs) {
      var self = this;
      this.loadApprovals();
      if (this._approvalTimer) clearInterval(this._approvalTimer);
      this._approvalTimer = setInterval(function () { self.loadApprovals(); }, intervalMs || 5000);
    },

    loadApprovals: async function () {
      try {
        var data = await FreecoAPI.get('/api/approvals');
        var list = Array.isArray(data) ? data : (data.approvals || data.pending || []);
        this.approvals = list.filter(function (a) {
          var s = (a.status || a.state || 'pending').toLowerCase();
          return s === 'pending' || s === 'waiting';
        });
      } catch (e) { /* keep whatever was already shown */ }
    },

    // decision: 'once' | 'always' | 'deny' | 'pause'
    answerApproval: async function (id, decision) {
      // "Pause" holds the request open and freezes agents, so the user can
      // think without the task racing ahead or being denied by a timeout.
      if (decision === 'pause') {
        try { await Alpine.store('app').toggleEmergencyFreeze(); } catch (e) { /* optional */ }
        return;
      }
      var path = decision === 'deny' ? '/reject' : '/approve';
      try {
        await FreecoAPI.post('/api/approvals/' + encodeURIComponent(id) + path,
          decision === 'always' ? { remember: true } : {});
        this.approvals = this.approvals.filter(function (a) {
          return (a.id || a.request_id) !== id;
        });
      } catch (e) {
        if (window.FreecoToast) FreecoToast.error('Could not record that answer: ' + e.message);
      }
    },

    // What the agent said it is trying to achieve. Falls back to the tool-level
    // description only when the agent supplied no purpose of its own.
    approvalAim: function (a) {
      return a.aim || a.description || ('use the ' + (a.tool_name || a.tool || 'tool'));
    },

    approvalConsequence: function (a) {
      return a.consequences || '';
    }
  };
};
