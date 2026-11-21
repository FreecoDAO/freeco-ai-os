// FreEco.ai — Chamber.
//
// One window for every conversation: agent chats and each person's
// conversation on WhatsApp, Telegram, email and every other channel. The
// server decides what this caller may see (crates/freeco-api/src/access.rs);
// this page only shows what it is given. Message text is always rendered
// with x-text, never as HTML: it comes from people outside.
'use strict';

var CHAMBER_CHANNEL_TAGS = {
  agent: 'AI',
  whatsapp: 'WA',
  telegram: 'TG',
  email: '@',
  discord: 'DC',
  slack: 'SL',
  signal: 'SG',
  matrix: 'MX',
  teams: 'MS',
  sms: 'SM',
  webchat: 'WC'
};

function chamberPage() {
  return {
    loading: true,
    loadError: '',
    conversations: [],
    channels: [],
    agents: [],
    filter: { channel: '', agent: '', unread: false },
    selectedId: '',
    selected: null,
    messages: [],
    threadLoading: false,
    threadError: '',
    draft: '',
    sending: false,
    _source: null,
    _timer: null,

    async init() {
      await this.load();
      this.connect();
    },

    destroy() {
      if (this._source) { this._source.close(); this._source = null; }
      if (this._timer) { clearInterval(this._timer); this._timer = null; }
    },

    query() {
      var parts = [];
      if (this.filter.channel) parts.push('channel=' + encodeURIComponent(this.filter.channel));
      if (this.filter.agent) parts.push('agent=' + encodeURIComponent(this.filter.agent));
      if (this.filter.unread) parts.push('unread=1');
      return parts.length ? '?' + parts.join('&') : '';
    },

    async load(quiet) {
      if (!quiet) this.loading = true;
      try {
        var res = await FreecoAPI.get('/api/chamber/conversations' + this.query());
        this.conversations = (res && res.conversations) || [];
        this.channels = (res && res.channels) || [];
        this.agents = (res && res.agents) || [];
        this.loadError = '';
      } catch (e) {
        this.loadError = 'Could not load the conversations: ' + e.message;
      }
      this.loading = false;
    },

    // Live updates: the server sends a conversation whenever one this caller
    // may see changes. Without a stream (an old browser, a proxy that buffers
    // it), poll instead.
    connect() {
      var self = this;
      if (typeof EventSource === 'undefined') { this.poll(); return; }
      var url = '/api/chamber/stream';
      var token = FreecoAPI.getToken();
      if (token) url += '?token=' + encodeURIComponent(token);
      try {
        this._source = new EventSource(url);
      } catch (e) { this.poll(); return; }
      this._source.onmessage = function (ev) {
        if (ev.data === 'ping') return;
        var data;
        try { data = JSON.parse(ev.data); } catch (e) { return; }
        if (!data || data.type !== 'conversation' || !data.conversation) return;
        self.upsert(data.conversation);
        if (data.conversation.id === self.selectedId) self.openThread(self.selectedId, true);
      };
      this._source.onerror = function () {
        if (self._source && self._source.readyState === 2) { self._source = null; self.poll(); }
      };
    },

    poll() {
      var self = this;
      if (this._timer) return;
      this._timer = setInterval(function () {
        if (!document.hidden) self.load(true);
      }, 5000);
    },

    // A changed conversation goes to the top, if it still passes the filters.
    upsert(c) {
      var rest = this.conversations.filter(function (x) { return x.id !== c.id; });
      if (this.matches(c)) rest.unshift(c);
      this.conversations = rest;
    },

    matches(c) {
      if (this.filter.channel && c.channel !== this.filter.channel) return false;
      if (this.filter.agent && c.agent_id !== this.filter.agent && c.agent_name !== this.filter.agent) return false;
      if (this.filter.unread && !(c.unread > 0) && c.id !== this.selectedId) return false;
      return true;
    },

    tag(channel) {
      return CHAMBER_CHANNEL_TAGS[channel] || String(channel || '?').slice(0, 2).toUpperCase();
    },

    channelLabel(channel) {
      if (channel === 'agent') return 'Agent chats';
      if (!channel) return '';
      return channel.charAt(0).toUpperCase() + channel.slice(1).replace(/_/g, ' ');
    },

    when(ts) {
      if (!ts) return '';
      var d = new Date(ts);
      if (isNaN(d.getTime())) return '';
      var now = new Date();
      if (d.toDateString() === now.toDateString()) {
        return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
      }
      return d.toLocaleDateString();
    },

    preview(c) {
      if (!c.last_message) return 'No messages yet';
      var who = c.last_message.role === 'assistant' ? (c.agent_name || 'Agent') + ': ' : '';
      return who + c.last_message.text;
    },

    async openThread(id, quiet) {
      this.selectedId = id;
      if (!quiet) { this.threadLoading = true; this.messages = []; this.draft = ''; }
      this.threadError = '';
      try {
        var res = await FreecoAPI.get('/api/chamber/conversations/' + encodeURIComponent(id) + '/messages');
        if (this.selectedId !== id) return;
        this.selected = res.conversation;
        this.messages = res.messages || [];
        // Opening it marked it read.
        var self = this;
        this.conversations = this.conversations.map(function (c) {
          return c.id === id ? Object.assign({}, c, { unread: 0 }) : c;
        });
        this.$nextTick(function () {
          var el = self.$refs.thread;
          if (el) el.scrollTop = el.scrollHeight;
        });
      } catch (e) {
        this.threadError = 'Could not open this conversation: ' + e.message;
      }
      this.threadLoading = false;
    },

    who(m) {
      if (!this.selected) return '';
      if (this.selected.channel === 'agent') {
        return m.role === 'user' ? 'You' : (this.selected.agent_name || 'Agent');
      }
      return m.role === 'user' ? this.selected.title : (this.selected.agent_name || 'Agent');
    },

    replyHint() {
      if (!this.selected) return '';
      if (!this.selected.can_reply) return 'You can read this conversation but not reply.';
      if (this.selected.channel === 'agent') return 'Message ' + (this.selected.agent_name || 'the agent');
      return 'Reply to ' + this.selected.title + ' on ' + this.channelLabel(this.selected.channel);
    },

    async send() {
      var text = (this.draft || '').trim();
      if (!text || !this.selected || this.sending) return;
      var id = this.selected.id;
      this.sending = true;
      try {
        var res = await FreecoAPI.post('/api/chamber/conversations/' + encodeURIComponent(id) + '/reply', { text: text });
        this.draft = '';
        if (res && res.response === '') FreecoToast.info('The agent chose not to answer.');
        await this.openThread(id, true);
        await this.load(true);
      } catch (e) {
        FreecoToast.error('Could not send: ' + e.message);
      }
      this.sending = false;
    }
  };
}
