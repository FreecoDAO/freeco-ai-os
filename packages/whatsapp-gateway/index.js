#!/usr/bin/env node

import http from 'node:http';
import { randomUUID, timingSafeEqual } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import makeWASocket, {
  useMultiFileAuthState,
  DisconnectReason,
  fetchLatestBaileysVersion,
  downloadMediaMessage,
} from '@whiskeysockets/baileys';
import QRCode from 'qrcode';
import pino from 'pino';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

// ---------------------------------------------------------------------------
// Config from environment
// ---------------------------------------------------------------------------
const PORT = parseInt(process.env.WHATSAPP_GATEWAY_PORT || '3009', 10);
const FREECO_AI_URL = (process.env.FREECO_AI_URL || process.env.FREECO_AI_URL || 'http://127.0.0.1:4200').replace(/\/+$/, '');
const DEFAULT_AGENT = process.env.FREECO_AI_DEFAULT_AGENT || process.env.FREECO_AI_DEFAULT_AGENT || 'assistant';
const GATEWAY_TOKEN = process.env.WHATSAPP_GATEWAY_TOKEN || '';
const FREECO_AI_API_KEY = process.env.FREECO_AI_API_KEY || process.env.FREECO_AI_API_KEY || '';

// ---------------------------------------------------------------------------
// Who the agent answers: [channels.whatsapp] allowed_users in config.toml
// ---------------------------------------------------------------------------
// Entries are phone numbers or WhatsApp ids; only their digits count, so
// "+41 79 123 45 67" and "41791234567" match alike. An empty list answers every
// direct message. Groups are answered only when the group's own id is listed,
// so an assistant on a personal number stays out of family and work groups.
// Read on every message, so edits apply without restarting the gateway.
const CONFIG_PATH = path.join(__dirname, '..', 'config.toml');

function digitsOf(jid) {
  return String(jid || '').replace(/@.*$/, '').replace(/:\d+$/, '').replace(/\D/g, '');
}

function loadAllowedUsers() {
  let text;
  try {
    text = fs.readFileSync(CONFIG_PATH, 'utf8');
  } catch {
    return [];
  }
  const section = text.split(/^\s*\[/m).find((s) => s.startsWith('channels.whatsapp]'));
  const list = section && section.match(/^\s*allowed_users\s*=\s*\[([\s\S]*?)\]/m);
  if (!list) return [];
  return (list[1].match(/"[^"]*"|'[^']*'|\d+/g) || []).map(digitsOf).filter(Boolean);
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------
let sock = null;          // Baileys socket
let sessionId = '';       // current session identifier
let qrDataUrl = '';       // latest QR code as data:image/png;base64,...
let connStatus = 'disconnected'; // disconnected | qr_ready | connected
let qrExpired = false;
let statusMessage = 'Not started';
let reconnectAttempt = 0; // exponential backoff counter
const MAX_RECONNECT_DELAY = 60_000; // cap at 60s
let reconnectTimer = null;  // the one pending reconnect, if any
let connectGeneration = 0;  // bumped by every startConnection()

// Reconnect later, replacing any reconnect already scheduled. Overlapping
// timers used to open two sockets on the same login.
function scheduleReconnect(delay) {
  clearTimeout(reconnectTimer);
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null;
    startConnection().catch((err) => {
      console.error('[gateway] Reconnect failed:', err.message);
      reconnectAttempt++;
      scheduleReconnect(Math.min(1000 * Math.pow(2, reconnectAttempt - 1), MAX_RECONNECT_DELAY));
    });
  }, delay);
}

// Baileys sometimes rejects a promise nobody awaits (a keep-alive on a socket
// that just closed, say). Node would exit on that and Freeco restarts the
// gateway only a few times, so log it and keep serving.
process.on('unhandledRejection', (err) => {
  console.error('[gateway] Unhandled rejection (kept running):', err?.stack || err);
});

// ---------------------------------------------------------------------------
// Baileys connection
// ---------------------------------------------------------------------------
async function startConnection() {
  // One socket at a time. Two sockets on the same login make WhatsApp drop
  // each in turn ("conflict: replaced") and deliver every message twice.
  const generation = ++connectGeneration;
  clearTimeout(reconnectTimer);
  reconnectTimer = null;
  if (sock) {
    const old = sock;
    sock = null;
    try {
      old.ev.removeAllListeners();
      old.end(undefined);
    } catch {
      // Already closed
    }
  }

  const logger = pino({ level: 'warn' });
  const authDir = path.join(__dirname, 'auth_store');

  const { state, saveCreds } = await useMultiFileAuthState(authDir);
  const { version } = await fetchLatestBaileysVersion();
  if (generation !== connectGeneration) return; // a newer start took over meanwhile

  sessionId = randomUUID();
  qrDataUrl = '';
  qrExpired = false;
  connStatus = 'disconnected';
  statusMessage = 'Connecting...';

  const thisSock = makeWASocket({
    version,
    auth: state,
    logger,
    printQRInTerminal: true,
    browser: ['FreEco.ai', 'Desktop', '1.0.0'],
  });
  sock = thisSock;

  // Save credentials whenever they update
  sock.ev.on('creds.update', saveCreds);

  // Connection state changes (QR code, connected, disconnected)
  sock.ev.on('connection.update', async (update) => {
    if (sock !== thisSock) return; // replaced by a newer socket
    const { connection, lastDisconnect, qr } = update;

    if (qr) {
      // New QR code generated — convert to data URL
      try {
        qrDataUrl = await QRCode.toDataURL(qr, { width: 256, margin: 2 });
        connStatus = 'qr_ready';
        qrExpired = false;
        // Reaching WhatsApp far enough to get a QR means the network is fine,
        // so earlier failures should not slow the reconnect after a scan.
        reconnectAttempt = 0;
        statusMessage = 'Scan this QR code with WhatsApp → Linked Devices';
        console.log('[gateway] QR code ready — waiting for scan');
      } catch (err) {
        console.error('[gateway] QR generation failed:', err.message);
      }
    }

    if (connection === 'close') {
      const statusCode = lastDisconnect?.error?.output?.statusCode;
      const reason = lastDisconnect?.error?.output?.payload?.message || 'unknown';
      console.log(`[gateway] Connection closed: ${reason} (${statusCode})`);

      if (statusCode === DisconnectReason.loggedOut) {
        // User logged out from phone — clear auth and stop (truly non-recoverable)
        connStatus = 'disconnected';
        statusMessage = 'Logged out. Generate a new QR code to reconnect.';
        qrDataUrl = '';
        sock = null;
        reconnectAttempt = 0;
        // Remove auth store so next connect gets a fresh QR
        const authPath = path.join(__dirname, 'auth_store');
        if (fs.existsSync(authPath)) {
          fs.rmSync(authPath, { recursive: true, force: true });
        }
      } else if (statusCode === DisconnectReason.restartRequired) {
        // 515 right after a QR scan is WhatsApp asking us to reconnect with the
        // new credentials. The phone waits only seconds for that, so reconnect
        // at once — a backoff delay here makes the phone show "try again later".
        console.log('[gateway] Restart required after pairing — reconnecting now');
        statusMessage = 'Linking...';
        connStatus = 'disconnected';
        scheduleReconnect(0);
      } else {
        // All other disconnect reasons are recoverable — reconnect with backoff
        // Covers: timedOut(408), connectionClosed(428),
        // connectionLost(408), connectionReplaced(440), badSession(500), etc.
        reconnectAttempt++;
        const delay = Math.min(1000 * Math.pow(2, reconnectAttempt - 1), MAX_RECONNECT_DELAY);
        console.log(`[gateway] Reconnecting in ${delay}ms (attempt ${reconnectAttempt})...`);
        statusMessage = `Reconnecting (attempt ${reconnectAttempt})...`;
        connStatus = 'disconnected';
        scheduleReconnect(delay);
      }
    }

    if (connection === 'open') {
      connStatus = 'connected';
      qrExpired = false;
      qrDataUrl = '';
      reconnectAttempt = 0;
      statusMessage = 'Connected to WhatsApp';
      console.log('[gateway] Connected to WhatsApp!');
    }
  });

  // Incoming messages → forward to FreEco.ai
  sock.ev.on('messages.upsert', async ({ messages, type }) => {
    if (sock !== thisSock) return; // replaced by a newer socket
    if (type !== 'notify') return;

    for (const msg of messages) {
      // Skip messages from self and status broadcasts
      if (msg.key.fromMe) continue;
      if (msg.key.remoteJid === 'status@broadcast') continue;

      const remoteJid = msg.key.remoteJid || '';
      const isGroup = remoteJid.endsWith('@g.us');

      const content = unwrapMessage(msg.message);
      const media = mediaOf(content);
      let text = content?.conversation
        || content?.extendedTextMessage?.text
        || media?.caption
        || '';
      if (!text && !media) continue; // Only skip truly empty messages

      // For groups: real sender is in participant; for DMs: it's remoteJid
      const senderJid = isGroup ? (msg.key.participant || '') : remoteJid;
      const phone = '+' + senderJid.replace(/@.*$/, '');
      const pushName = msg.pushName || phone;

      // Answer only the people (and groups) listed in allowed_users
      const allowed = loadAllowedUsers();
      if (isGroup) {
        if (!allowed.includes(digitsOf(remoteJid))) continue;
      } else if (allowed.length > 0) {
        // A sender may arrive as a privacy id (@lid); Baileys then carries the
        // phone-number JID separately, so either one can match.
        const ids = [remoteJid, msg.key.senderPn, msg.key.remoteJidAlt].map(digitsOf).filter(Boolean);
        if (!ids.some((id) => allowed.includes(id))) {
          console.log(`[gateway] Not answering ${pushName} (${phone}): not in allowed_users`);
          continue;
        }
      }

      // A voice note becomes its transcript; an image, video or document is
      // uploaded to the agent and attached. Done after the allowlist, so
      // nothing is downloaded for people the agent does not answer.
      let attachments = [];
      if (media) {
        try {
          ({ text, attachments } = await prepareMedia(msg, media, text, logger, thisSock));
        } catch (err) {
          console.error('[gateway] Media handling failed:', err.message);
          text = withNote(text, `[${MEDIA_LABELS[media.kind]} received, but it could not be read (${err.message})]`);
        }
      }

      const metadata = {
        channel: 'whatsapp',
        sender: phone,
        sender_name: pushName,
      };
      if (isGroup) {
        metadata.group_jid = remoteJid;
        metadata.is_group = true;
        console.log(`[gateway] Group msg from ${pushName} (${phone}) in ${remoteJid}: ${text.substring(0, 80)}`);
      } else {
        console.log(`[gateway] Incoming from ${pushName} (${phone}): ${text.substring(0, 80)}`);
      }

      // Forward to FreEco.ai agent
      try {
        const response = await forwardToFreecoAi(text, phone, pushName, metadata, attachments);
        if (!response) console.log(`[gateway] Agent ${DEFAULT_AGENT} returned no reply text`);
        if (response && sock) {
          // Reply in the same chat: group → group, DM → DM. Use the chat's own
          // JID; a DM can come from a privacy id (@lid) that is not a phone
          // number, so rebuilding it as <digits>@s.whatsapp.net misdelivers.
          const replyJid = remoteJid;
          await sock.sendMessage(replyJid, { text: response });
          console.log(`[gateway] Replied to ${pushName}${isGroup ? ' in group ' + remoteJid : ''}`);
        }
      } catch (err) {
        console.error(`[gateway] Forward/reply failed:`, err.message);
      }
    }
  });
}

// ---------------------------------------------------------------------------
// Media: voice notes, images, videos, documents
// ---------------------------------------------------------------------------
const MAX_MEDIA_BYTES = 16 * 1024 * 1024;

const EXTENSIONS = {
  'image/jpeg': 'jpg',
  'image/png': 'png',
  'image/webp': 'webp',
  'image/gif': 'gif',
  'video/mp4': 'mp4',
  'video/3gpp': '3gp',
  'audio/ogg': 'ogg',
  'audio/mpeg': 'mp3',
  'audio/mp4': 'm4a',
  'application/pdf': 'pdf',
};

const MEDIA_LABELS = {
  audio: 'Voice note',
  image: 'Image',
  video: 'Video',
  document: 'Document',
  sticker: 'Sticker',
};

// WhatsApp wraps some messages (disappearing, view-once, a document with a
// caption); the content sits inside the wrapper.
function unwrapMessage(message) {
  let m = message;
  for (let depth = 0; depth < 5 && m; depth++) {
    const inner = m.ephemeralMessage?.message
      || m.viewOnceMessage?.message
      || m.viewOnceMessageV2?.message
      || m.documentWithCaptionMessage?.message;
    if (!inner) break;
    m = inner;
  }
  return m;
}

// A protobuf length may arrive as a number or as a Long.
function toNumber(value) {
  const n = typeof value?.toNumber === 'function' ? value.toNumber() : Number(value || 0);
  return Number.isFinite(n) ? n : 0;
}

// The media in a message, if any, with what WhatsApp says about it.
function mediaOf(content) {
  if (!content) return null;
  const kinds = [
    ['audio', content.audioMessage],
    ['image', content.imageMessage],
    ['video', content.videoMessage],
    ['document', content.documentMessage],
    ['sticker', content.stickerMessage],
  ];
  for (const [kind, node] of kinds) {
    if (!node) continue;
    return {
      kind,
      mimetype: String(node.mimetype || '').split(';')[0].trim() || 'application/octet-stream',
      fileName: node.fileName || '',
      caption: node.caption || '',
      size: toNumber(node.fileLength),
    };
  }
  return null;
}

// A safe file name: the sender's own for documents, a made-up one otherwise.
function fileNameFor(media, messageId) {
  const ext = EXTENSIONS[media.mimetype] || 'bin';
  const name = media.fileName || `whatsapp-${media.kind}-${messageId || Date.now()}.${ext}`;
  return path.basename(name).replace(/[\\/:*?"<>|\x00-\x1f]/g, '_').slice(0, 120) || `file.${ext}`;
}

function withNote(caption, note) {
  return caption ? `${note}\n${caption}` : note;
}

function megabytes(bytes) {
  return (bytes / (1024 * 1024)).toFixed(1);
}

// Turn a message's media into something the agent can use: a voice note
// becomes its transcript, anything else is uploaded and attached. When a step
// fails the text says so, so the agent never gets a silent placeholder.
async function prepareMedia(msg, media, caption, logger, socket) {
  const label = MEDIA_LABELS[media.kind];
  if (media.kind === 'sticker') return { text: caption || '[Sticker received]', attachments: [] };

  const name = fileNameFor(media, msg.key.id);
  const tooBig = (bytes) => ({
    text: withNote(caption, `[${label} received: ${name}, ${megabytes(bytes)} MB, over the 16 MB limit, so it was not passed on]`),
    attachments: [],
  });
  if (media.size > MAX_MEDIA_BYTES) return tooBig(media.size);

  let buffer;
  try {
    buffer = await downloadMediaMessage(msg, 'buffer', {}, {
      logger,
      reuploadRequest: socket.updateMediaMessage,
    });
  } catch (err) {
    console.error(`[gateway] ${label} download failed:`, err.message);
    return {
      text: withNote(caption, `[${label} received: ${name}, but it could not be downloaded (${err.message})]`),
      attachments: [],
    };
  }
  if (buffer.length > MAX_MEDIA_BYTES) return tooBig(buffer.length);

  if (media.kind === 'audio') {
    try {
      const transcript = await transcribeAudio(buffer, media.mimetype);
      return { text: withNote(caption, `🎤 Voice note (transcribed): ${transcript}`), attachments: [] };
    } catch (err) {
      console.error('[gateway] Transcription failed:', err.message);
      return {
        text: withNote(caption, `🎤 Voice note received, but it could not be transcribed (${err.message}).`),
        attachments: [],
      };
    }
  }

  try {
    const attachment = await uploadToAgent(buffer, name, media.mimetype);
    return { text: caption || `Sent a file: ${name}`, attachments: [attachment] };
  } catch (err) {
    console.error(`[gateway] ${label} upload failed:`, err.message);
    return {
      text: withNote(caption, `[${label} received: ${name}, but it could not be passed on (${err.message})]`),
      attachments: [],
    };
  }
}

// POST the audio to FreEco.ai's transcription endpoint. It stores nothing.
async function transcribeAudio(buffer, mimetype) {
  const res = await fetch(`${FREECO_AI_URL}/api/transcribe`, {
    method: 'POST',
    headers: authHeaders({ 'Content-Type': mimetype || 'audio/ogg' }),
    body: buffer,
    signal: AbortSignal.timeout(120_000),
  });
  const body = await res.text();
  if (!res.ok) throw new Error(`FreEco.ai answered ${res.status}: ${body.slice(0, 200)}`);
  const transcript = String(JSON.parse(body).text || '').trim();
  if (!transcript) throw new Error('the transcript was empty');
  return transcript;
}

// Upload a file to the agent's workspace; returns what /message expects in
// `attachments`.
async function uploadToAgent(buffer, filename, mimetype) {
  const agentId = await resolveAgentId();
  const form = new FormData();
  form.append('file', new Blob([buffer], { type: mimetype || 'application/octet-stream' }), filename);
  form.append('filename', filename);
  const res = await fetch(`${FREECO_AI_URL}/api/agents/${encodeURIComponent(agentId)}/upload`, {
    method: 'POST',
    headers: authHeaders(),
    body: form,
    signal: AbortSignal.timeout(60_000),
  });
  const body = await res.text();
  if (!res.ok) {
    if (res.status === 404) agentIdCache = ''; // the agent was respawned under a new id
    throw new Error(`FreEco.ai answered ${res.status}: ${body.slice(0, 200)}`);
  }
  const data = JSON.parse(body);
  return {
    file_id: data.file_id,
    filename: data.filename || filename,
    content_type: data.content_type || mimetype,
  };
}

// ---------------------------------------------------------------------------
// Forward incoming message to FreEco.ai API, return agent response
// ---------------------------------------------------------------------------
function authHeaders(extra = {}) {
  const headers = { ...extra };
  if (FREECO_AI_API_KEY) headers.Authorization = ['Bearer', FREECO_AI_API_KEY].join(' ');
  return headers;
}

// /api/agents/{id}/message wants the agent's id, while the config names the
// agent. Look the id up by name once and reuse it; fall back to the name.
let agentIdCache = '';
function resolveAgentId() {
  if (agentIdCache) return Promise.resolve(agentIdCache);
  return new Promise((resolve) => {
    const url = new URL(`${FREECO_AI_URL}/api/agents`);
    const req = http.request(
      {
        hostname: url.hostname,
        port: url.port || 4200,
        path: url.pathname,
        method: 'GET',
        headers: authHeaders(),
        timeout: 10_000,
      },
      (res) => {
        let body = '';
        res.on('data', (chunk) => (body += chunk));
        res.on('end', () => {
          try {
            const data = JSON.parse(body);
            const agents = Array.isArray(data) ? data : data.agents || [];
            const hit = agents.find((a) => a && (a.id === DEFAULT_AGENT || a.name === DEFAULT_AGENT));
            if (hit?.id) agentIdCache = String(hit.id);
          } catch {
            // Not JSON — fall back to the name below
          }
          if (!agentIdCache) {
            console.error(`[gateway] No agent named "${DEFAULT_AGENT}" in GET /api/agents; using the name as its id`);
          }
          resolve(agentIdCache || DEFAULT_AGENT);
        });
      },
    );
    req.on('error', () => resolve(DEFAULT_AGENT));
    req.on('timeout', () => {
      req.destroy();
      resolve(DEFAULT_AGENT);
    });
    req.end();
  });
}

async function forwardToFreecoAi(text, phone, pushName, metadata, attachments = []) {
  const agentId = await resolveAgentId();
  return new Promise((resolve, reject) => {
    const payload = JSON.stringify({
      message: text,
      metadata: metadata || {
        channel: 'whatsapp',
        sender: phone,
        sender_name: pushName,
      },
      ...(attachments.length > 0 ? { attachments } : {}),
    });

    const url = new URL(`${FREECO_AI_URL}/api/agents/${encodeURIComponent(agentId)}/message`);

    const req = http.request(
      {
        hostname: url.hostname,
        port: url.port || 4200,
        path: url.pathname,
        method: 'POST',
        headers: authHeaders({
          'Content-Type': 'application/json',
          'Content-Length': Buffer.byteLength(payload),
        }),
        timeout: 120_000, // LLM calls can be slow
      },
      (res) => {
        let body = '';
        res.on('data', (chunk) => (body += chunk));
        res.on('end', () => {
          if (res.statusCode >= 300) {
            // Wrong or stale id (the agent was respawned): look it up again next time
            agentIdCache = '';
            reject(new Error(`FreEco.ai answered ${res.statusCode}: ${body.slice(0, 300)}`));
            return;
          }
          try {
            const data = JSON.parse(body);
            // The /api/agents/{id}/message endpoint returns { response: "..." }
            resolve(data.response || data.message || data.text || '');
          } catch {
            resolve(body.trim() || '');
          }
        });
      },
    );

    req.on('error', reject);
    req.on('timeout', () => {
      req.destroy();
      reject(new Error('FreEco.ai API timeout'));
    });
    req.write(payload);
    req.end();
  });
}

// ---------------------------------------------------------------------------
// Send a message via Baileys (called by FreEco.ai for outgoing)
// ---------------------------------------------------------------------------
async function sendMessage(to, text) {
  if (!sock || connStatus !== 'connected') {
    throw new Error('WhatsApp not connected');
  }

  // If already a full JID (group or user), use as-is; otherwise normalize phone → JID
  const jid = to.includes('@') ? to : to.replace(/^\+/, '') + '@s.whatsapp.net';

  await sock.sendMessage(jid, { text });
}

// ---------------------------------------------------------------------------
// HTTP server
// ---------------------------------------------------------------------------
function parseBody(req) {
  return new Promise((resolve, reject) => {
    let body = '';
    req.on('data', (chunk) => (body += chunk));
    req.on('end', () => {
      try {
        resolve(body ? JSON.parse(body) : {});
      } catch (e) {
        reject(new Error('Invalid JSON'));
      }
    });
    req.on('error', reject);
  });
}

function jsonResponse(res, status, data) {
  const body = JSON.stringify(data);
  res.writeHead(status, {
    'Content-Type': 'application/json',
    'Content-Length': Buffer.byteLength(body),
    'Access-Control-Allow-Origin': FREECO_AI_URL,
    'Vary': 'Origin',
  });
  res.end(body);
}

function hasValidToken(req) {
  const authorization = req.headers.authorization || '';
  const supplied = authorization.startsWith('Bearer ') ? authorization.slice(7) : '';
  if (!GATEWAY_TOKEN || !supplied) return false;
  const expected = Buffer.from(GATEWAY_TOKEN);
  const actual = Buffer.from(supplied);
  return expected.length === actual.length && timingSafeEqual(expected, actual);
}

const server = http.createServer(async (req, res) => {
  // CORS preflight
  if (req.method === 'OPTIONS') {
    if (req.headers.origin !== FREECO_AI_URL) {
      return jsonResponse(res, 403, { error: 'Untrusted origin' });
    }
    res.writeHead(204, {
      'Access-Control-Allow-Origin': FREECO_AI_URL,
      'Access-Control-Allow-Methods': 'GET, POST, OPTIONS',
      'Access-Control-Allow-Headers': 'Authorization, Content-Type',
      'Vary': 'Origin',
    });
    return res.end();
  }

  const url = new URL(req.url, `http://localhost:${PORT}`);
  const pathname = url.pathname;

  try {
    if (!hasValidToken(req)) {
      return jsonResponse(res, 401, { error: 'Unauthorized' });
    }

    // POST /login/start — start Baileys connection, return QR
    if (req.method === 'POST' && pathname === '/login/start') {
      // If already connected, just return success
      if (connStatus === 'connected') {
        return jsonResponse(res, 200, {
          qr_data_url: '',
          session_id: sessionId,
          message: 'Already connected to WhatsApp',
          connected: true,
        });
      }

      // Start a new connection (resets any existing)
      await startConnection();

      // Wait briefly for QR to generate (Baileys emits it quickly)
      let waited = 0;
      while (!qrDataUrl && connStatus !== 'connected' && waited < 15_000) {
        await new Promise((r) => setTimeout(r, 300));
        waited += 300;
      }

      return jsonResponse(res, 200, {
        qr_data_url: qrDataUrl,
        session_id: sessionId,
        message: statusMessage,
        connected: connStatus === 'connected',
      });
    }

    // GET /login/status — poll for connection status
    if (req.method === 'GET' && pathname === '/login/status') {
      return jsonResponse(res, 200, {
        connected: connStatus === 'connected',
        message: statusMessage,
        expired: qrExpired,
      });
    }

    // POST /message/send — send outgoing message via Baileys
    if (req.method === 'POST' && pathname === '/message/send') {
      const body = await parseBody(req);
      const { to, text } = body;

      if (!to || !text) {
        return jsonResponse(res, 400, { error: 'Missing "to" or "text" field' });
      }

      await sendMessage(to, text);
      return jsonResponse(res, 200, { success: true, message: 'Sent' });
    }

    // GET /health — health check
    if (req.method === 'GET' && pathname === '/health') {
      return jsonResponse(res, 200, {
        status: 'ok',
        connected: connStatus === 'connected',
        session_id: sessionId || null,
      });
    }

    // 404
    jsonResponse(res, 404, { error: 'Not found' });
  } catch (err) {
    console.error(`[gateway] ${req.method} ${pathname} error:`, err.message);
    jsonResponse(res, 500, { error: err.message });
  }
});

server.listen(PORT, '127.0.0.1', () => {
  console.log(`[gateway] WhatsApp Web gateway listening on http://127.0.0.1:${PORT}`);
  console.log(`[gateway] FreEco.ai URL: ${FREECO_AI_URL}`);
  console.log(`[gateway] Default agent: ${DEFAULT_AGENT}`);

  // Auto-connect if credentials already exist from a previous session
  const credsPath = path.join(__dirname, 'auth_store', 'creds.json');
  if (fs.existsSync(credsPath)) {
    console.log('[gateway] Found existing credentials — auto-connecting...');
    startConnection().catch((err) => {
      console.error('[gateway] Auto-connect failed:', err.message);
      statusMessage = 'Auto-connect failed. Use POST /login/start to retry.';
    });
  } else {
    console.log('[gateway] No credentials found. Waiting for POST /login/start to begin QR flow...');
  }
});

// Graceful shutdown
process.on('SIGINT', () => {
  console.log('\n[gateway] Shutting down...');
  if (sock) sock.end();
  server.close(() => process.exit(0));
});

process.on('SIGTERM', () => {
  if (sock) sock.end();
  server.close(() => process.exit(0));
});
