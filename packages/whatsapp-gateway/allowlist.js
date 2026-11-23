// Who the agent may talk to. Pure functions, no dependencies, so they can be
// tested on their own (test/allowlist.test.js).
//
// The list comes from FreEco.ai's config ([channels.whatsapp] allowed_users
// and owner_number), passed in as WHATSAPP_ALLOWED_NUMBERS and
// WHATSAPP_ALLOWED_GROUPS. The daemon checks the same list again; this file
// mirrors crates/freeco-channels/src/whatsapp_allowlist.rs and the two must
// normalise exactly alike.
//
// Canonical forms:
//   number      +41791234567       (from "+41 79 123 45 67", "0041…",
//                                    "41791234567@s.whatsapp.net", "…:3@…", "…@c.us")
//   group       120363…@g.us
//   privacy id  123…@lid           (matches only if listed as such)
// A national number with one leading zero ("079 …") has no country code and
// is rejected, never guessed. Empty list: nobody. No wildcard.

const MIN_DIGITS = 6;
const MAX_DIGITS = 15;

function stripDevice(local) {
  return String(local).split(':')[0];
}

/** A phone number as "+<digits>", or null if it is not a usable international number. */
export function normalizeNumber(raw) {
  const s = String(raw ?? '').trim();
  let local = s;
  const at = s.indexOf('@');
  if (at !== -1) {
    const domain = s.slice(at + 1).trim().toLowerCase();
    if (domain !== 's.whatsapp.net' && domain !== 'c.us') return null;
    local = stripDevice(s.slice(0, at));
  }
  local = local.trim();
  const rest = local.startsWith('+') ? local.slice(1) : local;
  if (/[^0-9 \-.()\/]/.test(rest)) return null;
  let digits = rest.replace(/[^0-9]/g, '');
  if (digits.startsWith('00')) digits = digits.slice(2);
  if (digits.startsWith('0')) return null;
  if (digits.length < MIN_DIGITS || digits.length > MAX_DIGITS) return null;
  return '+' + digits;
}

/** The canonical form of a chat id or list entry, or null. */
export function normalizeChat(raw) {
  const s = String(raw ?? '').trim();
  const at = s.indexOf('@');
  if (at === -1) return normalizeNumber(s);
  const domain = s.slice(at + 1).trim().toLowerCase();
  const local = s.slice(0, at);
  if (domain === 'g.us') {
    const id = local.replace(/\s/g, '');
    return id && /^[0-9-]+$/.test(id) ? `${id}@g.us` : null;
  }
  if (domain === 'lid') {
    const id = stripDevice(local).trim();
    return id && /^[0-9]+$/.test(id) ? `${id}@lid` : null;
  }
  return normalizeNumber(s);
}

/**
 * The allowlist from the two env values (comma-separated). Returns the set of
 * canonical entries and the entries that were not understood.
 */
export function parseAllowlist(numbersEnv, groupsEnv) {
  const entries = new Set();
  const rejected = [];
  for (const value of [numbersEnv, groupsEnv]) {
    for (const raw of String(value ?? '').split(',').map((x) => x.trim()).filter(Boolean)) {
      const canonical = normalizeChat(raw);
      if (canonical) entries.add(canonical);
      else rejected.push(raw);
    }
  }
  return { entries, rejected };
}

/** Whether the chat (number, user JID, group JID, @lid) is on the list. */
export function isAllowed(list, chatId) {
  const canonical = normalizeChat(chatId);
  return canonical !== null && list.entries.has(canonical);
}

/**
 * Decide on an incoming message before anything else is done with it (no
 * download, no forward, no read receipt). `key` is the Baileys message key.
 * In a group the group itself must be listed; in a direct chat, the sender.
 * Returns { allowed, chat, isGroup }; `chat` is what was checked.
 */
export function inboundDecision(list, key) {
  const remoteJid = String(key?.remoteJid || '');
  const isGroup = remoteJid.endsWith('@g.us');
  if (isGroup) return { allowed: isAllowed(list, remoteJid), chat: remoteJid, isGroup };
  // A direct chat can come from a privacy id (@lid); newer WhatsApp versions
  // also give the phone-number JID (senderPn). Either being listed allows it.
  const candidates = [remoteJid, key?.senderPn, key?.participantPn].filter(Boolean);
  const allowed = candidates.some((c) => isAllowed(list, c));
  return { allowed, chat: remoteJid, isGroup };
}

/** The JID to send to for a "to" value, or null if it is not a valid chat. */
export function jidFor(to) {
  const canonical = normalizeChat(to);
  if (!canonical) return null;
  if (canonical.startsWith('+')) return canonical.slice(1) + '@s.whatsapp.net';
  return canonical;
}
