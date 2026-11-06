// FreEco.ai API Client — Fetch wrapper, WebSocket manager, auth injection, toast notifications
'use strict';

// ── Toast Notification System ──
var FreecoToast = (function() {
  var _container = null;
  var _toastId = 0;

  function getContainer() {
    if (!_container) {
      _container = document.getElementById('toast-container');
      if (!_container) {
        _container = document.createElement('div');
        _container.id = 'toast-container';
        _container.className = 'toast-container';
        document.body.appendChild(_container);
      }
    }
    return _container;
  }

  function toast(message, type, duration) {
    type = type || 'info';
    duration = duration || 4000;
    var id = ++_toastId;
    var el = document.createElement('div');
    el.className = 'toast toast-' + type;
    el.setAttribute('data-toast-id', id);

    var msgSpan = document.createElement('span');
    msgSpan.className = 'toast-msg';
    msgSpan.textContent = message;
    el.appendChild(msgSpan);

    var closeBtn = document.createElement('button');
    closeBtn.className = 'toast-close';
    closeBtn.textContent = '\u00D7';
    closeBtn.onclick = function() { dismissToast(el); };
    el.appendChild(closeBtn);

    el.onclick = function(e) { if (e.target === el) dismissToast(el); };
    getContainer().appendChild(el);

    // Auto-dismiss
    if (duration > 0) {
      setTimeout(function() { dismissToast(el); }, duration);
    }
    return id;
  }

  function dismissToast(el) {
    if (!el || el.classList.contains('toast-dismiss')) return;
    el.classList.add('toast-dismiss');
    setTimeout(function() { if (el.parentNode) el.parentNode.removeChild(el); }, 300);
  }

  function success(msg, duration) { return toast(msg, 'success', duration); }
  function error(msg, duration) { return toast(msg, 'error', duration || 6000); }
  function warn(msg, duration) { return toast(msg, 'warn', duration || 5000); }
  function info(msg, duration) { return toast(msg, 'info', duration); }

  // Styled confirmation modal — replaces native confirm()
  function confirm(title, message, onConfirm) {
    var overlay = document.createElement('div');
    overlay.className = 'confirm-overlay';

    var modal = document.createElement('div');
    modal.className = 'confirm-modal';

    var titleEl = document.createElement('div');
    titleEl.className = 'confirm-title';
    titleEl.textContent = title;
    modal.appendChild(titleEl);

    var msgEl = document.createElement('div');
    msgEl.className = 'confirm-message';
    msgEl.textContent = message;
    modal.appendChild(msgEl);

    var actions = document.createElement('div');
    actions.className = 'confirm-actions';

    var cancelBtn = document.createElement('button');
    cancelBtn.className = 'btn btn-ghost confirm-cancel';
    cancelBtn.textContent = 'Cancel';
    actions.appendChild(cancelBtn);

    var okBtn = document.createElement('button');
    okBtn.className = 'btn btn-danger confirm-ok';
    okBtn.textContent = 'Confirm';
    actions.appendChild(okBtn);

    modal.appendChild(actions);
    overlay.appendChild(modal);

    function close() { if (overlay.parentNode) overlay.parentNode.removeChild(overlay); document.removeEventListener('keydown', onKey); }
    cancelBtn.onclick = close;
    okBtn.onclick = function() { close(); if (onConfirm) onConfirm(); };
    overlay.addEventListener('click', function(e) { if (e.target === overlay) close(); });

    function onKey(e) { if (e.key === 'Escape') close(); }
    document.addEventListener('keydown', onKey);

    document.body.appendChild(overlay);
    okBtn.focus();
  }

  return {
    toast: toast,
    success: success,
    error: error,
    warn: warn,
    info: info,
    confirm: confirm
  };
})();

// ── Friendly Error Messages ──
function friendlyError(status, serverMsg) {
  if (status === 0 || !status) return 'Cannot reach daemon — is freeco running?';
  if (status === 401) return 'Not authorized — check your API key';
  if (status === 403) return 'Permission denied';
  if (status === 404) return serverMsg || 'Resource not found';
  if (status === 429) return 'Rate limited — slow down and try again';
  if (status === 413) return 'Request too large';
  if (status === 500) return 'Server error — check daemon logs';
  if (status === 502 || status === 503) return 'Daemon unavailable — is it running?';
  return serverMsg || 'Unexpected error (' + status + ')';
}

// ── API Client ──
var FreecoAPI = (function() {
  var BASE = window.location.origin;
  var WS_BASE = BASE.replace(/^http/, 'ws');
  var _authToken = '';

  // Connection state tracking
  var _connectionState = 'connected';
  var _reconnectAttempt = 0;
  var _connectionListeners = [];

  function setAuthToken(token) { _authToken = token; }

  function headers() {
    var h = { 'Content-Type': 'application/json' };
    if (_authToken) h['Authorization'] = 'Bearer ' + _authToken;
    return h;
  }

  function setConnectionState(state) {
    if (_connectionState === state) return;
    _connectionState = state;
    _connectionListeners.forEach(function(fn) { fn(state); });
  }

  function onConnectionChange(fn) { _connectionListeners.push(fn); }

  function request(method, path, body) {
    var opts = { method: method, headers: headers() };
    if (body !== undefined) opts.body = JSON.stringify(body);
    return fetch(BASE + path, opts).then(function(r) {
      if (_connectionState !== 'connected') setConnectionState('connected');
      if (!r.ok) {
        // On 401, auto-show auth prompt so the user can re-enter their key
        if (r.status === 401 && typeof Alpine !== 'undefined') {
          try {
            var store = Alpine.store('app');
            if (store && !store.showAuthPrompt) {
              _authToken = '';
              localStorage.removeItem('freeco-api-key');
              store.showAuthPrompt = true;
            }
          } catch(e2) { /* ignore Alpine errors */ }
        }
        return r.text().then(function(text) {
          var msg = '';
          try {
            var json = JSON.parse(text);
            msg = json.error || r.statusText;
          } catch(e) {
            msg = r.statusText;
          }
          throw new Error(friendlyError(r.status, msg));
        });
      }
      var ct = r.headers.get('content-type') || '';
      if (ct.indexOf('application/json') >= 0) return r.json();
      return r.text().then(function(t) {
        try { return JSON.parse(t); } catch(e) { return { text: t }; }
      });
    }).catch(function(e) {
      if (e.name === 'TypeError' && e.message.includes('Failed to fetch')) {
        setConnectionState('disconnected');
        throw new Error('Cannot connect to daemon — is freeco running?');
      }
      throw e;
    });
  }

  function get(path) { return request('GET', path); }
  function post(path, body) { return request('POST', path, body); }
  function put(path, body) { return request('PUT', path, body); }
  function patch(path, body) { return request('PATCH', path, body); }
  function del(path) { return request('DELETE', path); }

  // WebSocket manager with auto-reconnect
  var _ws = null;
  var _wsCallbacks = {};
  var _wsConnected = false;
  var _wsAgentId = null;
  var _reconnectTimer = null;
  var _reconnectAttempts = 0;
  var MAX_RECONNECT = 5;

  function wsConnect(agentId, callbacks) {
    wsDisconnect();
    _wsCallbacks = callbacks || {};
    _wsAgentId = agentId;
    _reconnectAttempts = 0;
    _doConnect(agentId);
  }

  function _doConnect(agentId) {
    try {
      var url = WS_BASE + '/api/agents/' + agentId + '/ws';
      if (_authToken) url += '?token=' + encodeURIComponent(_authToken);
      var socket = new WebSocket(url);
      _ws = socket;

      socket.onopen = function() {
        // Guard: ignore if this socket was superseded by a newer connection
        if (_ws !== socket) return;
        _wsConnected = true;
        _reconnectAttempts = 0;
        setConnectionState('connected');
        if (_reconnectAttempt > 0) {
          FreecoToast.success('Reconnected');
          _reconnectAttempt = 0;
        }
        if (_wsCallbacks.onOpen) _wsCallbacks.onOpen();
      };

      socket.onmessage = function(e) {
        try {
          var data = JSON.parse(e.data);
        } catch(parseErr) {
          return; // Ignore malformed JSON frames
        }
        // The server closes a socket nobody has used for 30 minutes. That is
        // housekeeping, not a failure: reconnect quietly instead of showing it.
        if (data && data.type === 'idle_timeout') { socket._idle = true; return; }
        // Dispatch outside try/catch so handler errors are not swallowed
        if (_wsCallbacks.onMessage) _wsCallbacks.onMessage(data);
      };

      socket.onclose = function(e) {
        // Guard: only update state if this is still the active socket.
        // A superseded socket closing must not null-out the new connection.
        if (_ws !== socket) return;
        _wsConnected = false;
        _ws = null;
        if (socket._idle && _wsAgentId) {
          _reconnectAttempts = 0;
          _doConnect(_wsAgentId);
          return;
        }
        if (_wsAgentId && _reconnectAttempts < MAX_RECONNECT && e.code !== 1000) {
          _reconnectAttempts++;
          _reconnectAttempt = _reconnectAttempts;
          setConnectionState('reconnecting');
          if (_reconnectAttempts === 1) {
            FreecoToast.warn('Connection lost, reconnecting...');
          }
          var delay = Math.min(1000 * Math.pow(2, _reconnectAttempts - 1), 10000);
          _reconnectTimer = setTimeout(function() { _doConnect(_wsAgentId); }, delay);
          return;
        }
        if (_wsAgentId && _reconnectAttempts >= MAX_RECONNECT) {
          setConnectionState('disconnected');
          FreecoToast.error('Connection lost — switched to HTTP mode', 0);
        }
        if (_wsCallbacks.onClose) _wsCallbacks.onClose();
      };

      socket.onerror = function() {
        // Guard: ignore errors from superseded sockets
        if (_ws !== socket) return;
        _wsConnected = false;
        if (_wsCallbacks.onError) _wsCallbacks.onError();
      };
    } catch(e) {
      _wsConnected = false;
    }
  }

  function wsDisconnect() {
    _wsAgentId = null;
    _reconnectAttempts = MAX_RECONNECT;
    if (_reconnectTimer) { clearTimeout(_reconnectTimer); _reconnectTimer = null; }
    if (_ws) { _ws.close(1000); _ws = null; }
    _wsConnected = false;
  }

  function wsSend(data) {
    if (_ws && _ws.readyState === WebSocket.OPEN) {
      _ws.send(JSON.stringify(data));
      return true;
    }
    return false;
  }

  function isWsConnected() { return _wsConnected; }

  // A connection of its own, for a window that talks to an agent alongside
  // the main chat.
  //
  // The page used to have one shared socket. The assistant popup and the main
  // chat both sent over it, so whichever connected last took every reply, and
  // a message typed in one window went to the other window's agent. Each
  // caller of this gets a separate socket, bound to one agent for its life.
  function agentSocket(agentId, callbacks) {
    var cbs = callbacks || {};
    var sock = null;
    var closed = false;
    var attempts = 0;
    var timer = null;
    var queue = [];

    function open() {
      if (closed) return;
      var url = WS_BASE + '/api/agents/' + agentId + '/ws';
      if (_authToken) url += '?token=' + encodeURIComponent(_authToken);
      var s;
      try { s = new WebSocket(url); } catch (e) { return; }
      sock = s;
      s.onopen = function() {
        if (sock !== s) return;
        attempts = 0;
        while (queue.length) s.send(queue.shift());
        if (cbs.onOpen) cbs.onOpen();
      };
      s.onmessage = function(e) {
        if (sock !== s) return;
        var data;
        try { data = JSON.parse(e.data); } catch (err) { return; }
        if (data && data.type === 'idle_timeout') { s._idle = true; return; }
        if (cbs.onMessage) cbs.onMessage(data);
      };
      s.onclose = function(e) {
        if (sock !== s) return;
        sock = null;
        if (closed) return;
        if (s._idle) { attempts = 0; open(); return; }
        if (e.code !== 1000 && attempts < MAX_RECONNECT) {
          attempts++;
          timer = setTimeout(open, Math.min(1000 * Math.pow(2, attempts - 1), 10000));
          return;
        }
        if (cbs.onClose) cbs.onClose();
      };
    }

    open();
    return {
      agentId: agentId,
      // Sent now if open, or as soon as the socket opens. False only once
      // this handle is closed, so the caller can fall back to HTTP.
      send: function(data) {
        if (closed) return false;
        var text = JSON.stringify(data);
        if (sock && sock.readyState === WebSocket.OPEN) { sock.send(text); return true; }
        queue.push(text);
        if (!sock) open();
        return true;
      },
      isOpen: function() { return !!sock && sock.readyState === WebSocket.OPEN; },
      close: function() {
        closed = true;
        queue = [];
        if (timer) { clearTimeout(timer); timer = null; }
        if (sock) { var s = sock; sock = null; s.close(1000); }
      }
    };
  }

  function getConnectionState() { return _connectionState; }

  function getToken() { return _authToken; }

  function upload(agentId, file) {
    var hdrs = {};
    if (_authToken) hdrs['Authorization'] = 'Bearer ' + _authToken;
	var form = new FormData();
    form.append('file', file);
    form.append('filename', file.name);
    return fetch(BASE + '/api/agents/' + agentId + '/upload', {
      method: 'POST',
      headers: hdrs,
      body: form
    }).then(function(r) {
      if (!r.ok) throw new Error('Upload failed');
      return r.json();
    });
  }

  return {
    setAuthToken: setAuthToken,
    getToken: getToken,
    get: get,
    post: post,
    put: put,
    patch: patch,
    del: del,
    delete: del,
    upload: upload,
    wsConnect: wsConnect,
    wsDisconnect: wsDisconnect,
    wsSend: wsSend,
    isWsConnected: isWsConnected,
    agentSocket: agentSocket,
    getConnectionState: getConnectionState,
    onConnectionChange: onConnectionChange
  };
})();
